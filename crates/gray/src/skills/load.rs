//! Skill file parsing + directory loading (split from `skills`).

use super::*;

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
        // no frontmatter → empty, body is whole file
        return Ok((SkillFrontmatter::default(), content.to_string()));
    }
    // find closing ---
    let after_open = &trimmed[3..];
    // skip first newline after opening
    let after_open = after_open
        .strip_prefix("\r\n")
        .or_else(|| after_open.strip_prefix('\n'))
        .unwrap_or(after_open);
    if let Some(end) = find_closing_delim(after_open) {
        let fm_str = &after_open[..end];
        let body = &after_open[end..];
        let body = body.strip_prefix("---").unwrap_or(body);
        let body = body
            .strip_prefix("\r\n")
            .or_else(|| body.strip_prefix('\n'))
            .unwrap_or(body)
            .to_string();
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
// Validation
// ---------------------------------------------------------------------------

fn validate_name(name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if name.len() > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {MAX_NAME_LENGTH} characters ({})",
            name.len()
        ));
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        errors.push(
            "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"
                .to_string(),
        );
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
        Some(d) if d.len() > MAX_DESCRIPTION_LENGTH => errors.push(format!(
            "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({})",
            d.len()
        )),
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

pub(crate) fn load_skill_from_file(
    file_path: &Path,
    source: &str,
) -> (Option<Skill>, Vec<Diagnostic>) {
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

// internal walker
pub(crate) fn load_skills_from_dir_internal(
    dir: &Path,
    source: &str,
    include_root_files: bool,
    matcher: &mut IgnoreMatcher,
    root_dir: &Path,
) -> LoadSkillsResult {
    let mut skills = Vec::new();
    let mut diagnostics = Vec::new();

    if !dir.exists() {
        return LoadSkillsResult {
            skills,
            diagnostics,
        };
    }

    add_ignore_rules(matcher, dir, root_dir);

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            return LoadSkillsResult {
                skills,
                diagnostics,
            };
        }
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
            fs::metadata(full_path)
                .map(|m| m.is_file())
                .unwrap_or(false)
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
        return LoadSkillsResult {
            skills,
            diagnostics,
        };
    }

    // Phase 2: scan children
    for (full_path, ft, is_symlink) in entry_list {
        let file_name = full_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
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
            let mut sub =
                load_skills_from_dir_internal(&full_path, source, false, matcher, root_dir);
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

    LoadSkillsResult {
        skills,
        diagnostics,
    }
}
