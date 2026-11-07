use std::fs;
use std::path::{Path, PathBuf};

pub struct Skill {
    pub name: String,
    pub description: String,
    pub source: String,
    pub file: PathBuf,
    pub body: String,
}

fn parse_skill(file: &Path, source: &str) -> Option<Skill> {
    let raw = fs::read_to_string(file).ok()?;
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let rest = &trimmed[3..];
    let end = rest.find("\n---")?;
    let fm = &rest[..end];
    let body = rest[end + 4..].trim_start_matches('\n').to_string();
    let mut name = None;
    let mut desc = None;
    for line in fm.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(v) = line.strip_prefix("description:") {
            desc = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    let desc = desc.filter(|d| !d.is_empty())?;
    let dir_name = file
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    Some(Skill {
        name: name.filter(|n| !n.is_empty()).unwrap_or(dir_name),
        description: desc,
        source: source.to_string(),
        file: file.to_path_buf(),
        body,
    })
}

fn scan_dir(
    dir: &Path,
    source: &str,
    out: &mut Vec<Skill>,
    seen: &mut std::collections::HashSet<PathBuf>,
) {
    let canon = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !seen.insert(canon) {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md")
            && let Some(s) = parse_skill(&path, source)
        {
            out.push(s);
        }
        if path.is_dir() {
            scan_dir(&path, source, out, seen);
        }
    }
}

fn claude_disabled_plugins(home: &Path) -> std::collections::HashSet<String> {
    let mut disabled = std::collections::HashSet::new();
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

pub fn discover(work_root: Option<&Path>, agent_dir: &Path) -> Vec<Skill> {
    let mut out = Vec::new();
    let mut seen_dirs = std::collections::HashSet::new();
    scan_dir(&agent_dir.join("skills"), "user", &mut out, &mut seen_dirs);
    if let Some(home) = dirs_home() {
        scan_dir(
            &home.join(".claude").join("skills"),
            "user",
            &mut out,
            &mut seen_dirs,
        );
        let opencode = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".config"))
            .join("opencode");
        if let Ok(entries) = fs::read_dir(&opencode) {
            let mut dirs: Vec<PathBuf> = vec![opencode.clone()];
            dirs.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
            for d in dirs {
                scan_dir(&d.join("skills"), "user", &mut out, &mut seen_dirs);
            }
        } else {
            scan_dir(&opencode.join("skills"), "user", &mut out, &mut seen_dirs);
        }
        scan_dir(
            &home.join(".agents").join("skills"),
            "user",
            &mut out,
            &mut seen_dirs,
        );
        for dir in claude_plugin_skill_dirs(&home) {
            scan_dir(&dir, "plugin", &mut out, &mut seen_dirs);
        }
    }
    if let Some(root) = work_root {
        let mut dir = Some(root.to_path_buf());
        while let Some(d) = dir {
            for cfg in [".gray", ".claude", ".agents"] {
                scan_dir(
                    &d.join(cfg).join("skills"),
                    "project",
                    &mut out,
                    &mut seen_dirs,
                );
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| {
        let real = fs::canonicalize(&s.file).unwrap_or_else(|_| s.file.clone());
        seen.insert(real)
    });
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
