//! Skill file parsing + directory loading (split from `skills`).

use super::*;

// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    disable_model_invocation: bool,
    args: Vec<String>,
}

/// Split a frontmatter `args:`/`arguments:` value into declared arg names.
/// Accepts comma- and/or whitespace-separated (`foo, bar`, `foo bar`,
/// `[foo, bar]`); surrounding quotes already stripped by the caller.
pub(crate) fn parse_declared_args(val: &str) -> Vec<String> {
    let t = val.trim();
    let t = t.strip_prefix('[').unwrap_or(t);
    let t = t.strip_suffix(']').unwrap_or(t);
    t.split([',', ' ', '\t'])
        .map(|s| {
            s.trim()
                .trim_matches('"')
                .trim_matches('\'')
                .trim_start_matches('-')
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
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
    let lines: Vec<&str> = s.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        i += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let key = line[..colon].trim().trim_matches('"').trim_matches('\'');
        let mut val = line[colon + 1..].trim().to_string();
        // Block scalars (`description: >` / `|` with optional chomping
        // `+`/`-`): gather the indented continuation lines YAML folds into
        // the value. Without this a folded description parses as the bare
        // marker (`">"`), silently breaking skill discovery text.
        // ponytail: chomping nuances ignored, descriptions are trimmed downstream.
        let folded = val == ">" || val == ">-" || val == ">+";
        let literal = val == "|" || val == "|-" || val == "|+";
        if folded || literal {
            let mut parts: Vec<String> = Vec::new();
            while i < lines.len() {
                let next = lines[i];
                if next.trim().is_empty() {
                    i += 1;
                    continue;
                }
                if !next.starts_with([' ', '\t']) {
                    break;
                }
                parts.push(next.trim().to_string());
                i += 1;
            }
            val = if folded {
                parts.join(" ")
            } else {
                parts.join("\n")
            };
        } else if (val.starts_with('"') && val.ends_with('"') && val.len() >= 2)
            || (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
        {
            // strip quotes
            val = val[1..val.len() - 1].to_string();
        }
        match key {
            "name" => fm.name = Some(val),
            "description" => fm.description = Some(val),
            "disable-model-invocation" | "disable_model_invocation" => {
                fm.disable_model_invocation = val == "true" || val == "True" || val == "TRUE"
            }
            "args" | "arguments" => {
                fm.args = parse_declared_args(&val);
            }
            _ => {}
        }
    }
    fm
}

// ---------------------------------------------------------------------------
// Core loaders
// ---------------------------------------------------------------------------

fn is_skill_md_file(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md")
}

pub(crate) fn load_skill_from_file(file_path: &Path, source: &str) -> Option<Skill> {
    let is_declared_skill = is_skill_md_file(file_path);

    let raw = match fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(_) => return None,
    };

    let frontmatter = match parse_frontmatter(&raw) {
        Ok((fm, _)) => fm,
        Err(_) => return None,
    };

    let has_description = frontmatter
        .description
        .as_ref()
        .map(|d| !d.trim().is_empty())
        .unwrap_or(false);

    if !is_declared_skill && !has_description {
        return None;
    }

    let skill_dir = file_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let parent_dir_name = skill_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let frontmatter_name = frontmatter.name.clone();
    let name = frontmatter_name.unwrap_or(parent_dir_name);

    if !has_description {
        return None;
    }

    let description = frontmatter.description.unwrap_or_default();

    Some(Skill {
        name,
        description,
        file_path: file_path.to_path_buf(),
        base_dir: skill_dir,
        disable_model_invocation: frontmatter.disable_model_invocation,
        source: source.to_string(),
        args: frontmatter.args.clone(),
    })
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

    if !dir.exists() {
        return LoadSkillsResult { skills };
    }

    add_ignore_rules(matcher, dir, root_dir);

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            return LoadSkillsResult { skills };
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
        if let Some(s) = load_skill_from_file(full_path, source) {
            skills.push(s);
        }
        return LoadSkillsResult { skills };
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
            continue;
        }

        if !is_file || !include_root_files || !file_name.ends_with(".md") {
            continue;
        }

        if let Some(s) = load_skill_from_file(&full_path, source) {
            skills.push(s);
        }
    }

    LoadSkillsResult { skills }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn declared_args_split_commas_and_spaces() {
        assert_eq!(parse_declared_args("env, force"), vec!["env", "force"]);
        assert_eq!(parse_declared_args("env force"), vec!["env", "force"]);
        assert_eq!(parse_declared_args("[env, force]"), vec!["env", "force"]);
        assert!(parse_declared_args("").is_empty());
    }

    #[test]
    fn frontmatter_args_land_on_skill() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            "---\nname: deploy\ndescription: test skill\nargs: env, force\n---\nBody"
        )
        .unwrap();
        let skill = load_skill_from_file(f.path(), "path");
        let skill = skill.expect("loads");
        assert_eq!(skill.args, vec!["env".to_string(), "force".to_string()]);
    }

    #[test]
    fn frontmatter_without_args_means_no_args() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "---\ndescription: test skill\n---\nBody").unwrap();
        let skill = load_skill_from_file(f.path(), "path");
        let skill = skill.expect("loads");
        assert!(skill.args.is_empty());
    }

    #[test]
    fn folded_description_joins_continuation_lines() {
        // ponytail-style `description: >` frontmatter: the indented lines
        // fold into one description (previously parsed as the bare `">"`).
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            "---\nname: ponytail\ndescription: >\n  Forces the laziest solution\n  that actually works.\nargs: lite, full\n---\nBody"
        )
        .unwrap();
        let skill = load_skill_from_file(f.path(), "path");
        let skill = skill.expect("loads");
        assert_eq!(
            skill.description,
            "Forces the laziest solution that actually works."
        );
        assert_eq!(skill.args, vec!["lite".to_string(), "full".to_string()]);
    }

    #[test]
    fn literal_and_chomped_markers_parse() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "---\ndescription: |-\n  line one\n  line two\n---\nBody").unwrap();
        let skill = load_skill_from_file(f.path(), "path");
        assert_eq!(skill.expect("loads").description, "line one\nline two");

        let mut g = tempfile::NamedTempFile::new().unwrap();
        writeln!(g, "---\ndescription: >-\n  folded here\n---\nBody").unwrap();
        let skill = load_skill_from_file(g.path(), "path");
        assert_eq!(skill.expect("loads").description, "folded here");
    }
}
