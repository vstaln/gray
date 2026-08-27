//! Prompt templates — port of `packages/coding-agent/src/core/prompt-templates.ts` (285 LOC).
//!
//! Discovery dirs (literal port):
//! - global: `agentDir/prompts/`  (e.g. `~/.gray/prompts` or `~/.pi/agent/prompts`)
//! - project: `cwd/.pi/prompts` or `cwd/.gray/prompts` (walks up to git root for the latter)
//! - explicit `promptPaths` (files or directories)
//!
//! Each `.md` file becomes a slash-command template: name = file stem, description from
//! frontmatter or first non-empty line, argument-hint from frontmatter.

use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub content: String,
    pub file_path: PathBuf,
    /// synthetic source label: "user" | "project" | "path"
    pub source: String,
    pub base_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LoadPromptTemplatesOptions {
    pub cwd: PathBuf,
    pub agent_dir: Option<PathBuf>,
    pub prompt_paths: Vec<PathBuf>,
    pub include_defaults: bool,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
// Frontmatter — simple line parser like pi's yaml parse (no serde_yaml dep)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Frontmatter {
    description: Option<String>,
    argument_hint: Option<String>,
}

fn parse_frontmatter(content: &str) -> (Frontmatter, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (Frontmatter::default(), content.to_string());
    }
    let after_open = &trimmed[3..];
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
        let mut fm = Frontmatter::default();
        for line in fm_str.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(colon) = line.find(':') {
                let key = line[..colon].trim().trim_matches('"').trim_matches('\'');
                let mut val = line[colon + 1..].trim().to_string();
                if (val.starts_with('"') && val.ends_with('"') && val.len() >= 2)
                    || (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
                {
                    val = val[1..val.len() - 1].to_string();
                }
                match key {
                    "description" => fm.description = Some(val),
                    "argument-hint" | "argument_hint" => fm.argument_hint = Some(val),
                    _ => {}
                }
            }
        }
        (fm, body)
    } else {
        (Frontmatter::default(), content.to_string())
    }
}

fn find_closing_delim(s: &str) -> Option<usize> {
    let mut off = 0usize;
    for line in s.lines() {
        if line.trim() == "---" {
            return Some(off);
        }
        off += line.len() + 1;
    }
    None
}

// ---------------------------------------------------------------------------
// parseCommandArgs — bash-style quoted splitter (literal port)
// ---------------------------------------------------------------------------

pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;
    for ch in args_string.chars() {
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                args.push(current.clone());
                current.clear();
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

// ---------------------------------------------------------------------------
// substituteArgs — literal port of TS regex logic
// Supports:
// - $1, $2, ...  |  $@ / $ARGUMENTS
// - ${N:-default} | ${@:-default} | ${ARGUMENTS:-default}
// - ${@:N}  |  ${@:N:L}
// ---------------------------------------------------------------------------

pub fn substitute_args(content: &str, args: &[String]) -> String {
    let all_args = args.join(" ");
    let mut out = String::new();
    let bytes = content.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        // peek ahead
        if i + 1 >= bytes.len() {
            out.push('$');
            i += 1;
            continue;
        }
        let next = bytes[i + 1];
        if next == b'{' {
            // try ${...}
            if let Some(close) = content[i..].find('}') {
                let inner = &content[i + 2..i + close];
                // Check for :- default form: ${TARGET:-default}
                if let Some(sep) = inner.find(":-") {
                    let target = &inner[..sep];
                    let default_val = &inner[sep + 2..];
                    let value: Option<String> = if target == "@" || target == "ARGUMENTS" {
                        Some(all_args.clone())
                    } else if let Ok(n) = target.parse::<usize>() {
                        if n >= 1 && n <= args.len() {
                            Some(args[n - 1].clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let replacement = match value {
                        Some(v) if !v.is_empty() => v,
                        _ => default_val.to_string(),
                    };
                    out.push_str(&replacement);
                    i += close + 1;
                    continue;
                }
                // Check for slice: @:N or @:N:L  (inner starts with "@:")
                if inner.starts_with("@:") {
                    let rest = &inner[2..];
                    let parts: Vec<&str> = rest.split(':').collect();
                    if let Ok(start_n) = parts[0].parse::<usize>() {
                        let start = start_n.saturating_sub(1);
                        // bash treats 0 as 1
                        // start already 0 for 1, so 0 stays 0
                        if start >= args.len() {
                            // out of range -> empty
                            if parts.len() == 2 {
                                // with length, still empty
                            }
                            i += close + 1;
                            continue;
                        }
                        let replacement = if parts.len() == 2 {
                            if let Ok(len) = parts[1].parse::<usize>() {
                                args.iter().skip(start).take(len).cloned().collect::<Vec<_>>().join(" ")
                            } else {
                                args.iter().skip(start).cloned().collect::<Vec<_>>().join(" ")
                            }
                        } else {
                            args.iter().skip(start).cloned().collect::<Vec<_>>().join(" ")
                        };
                        out.push_str(&replacement);
                        i += close + 1;
                        continue;
                    }
                }
                // Not a recognized ${} form -> treat as literal
                out.push_str(&content[i..i + close + 1]);
                i += close + 1;
                continue;
            } else {
                out.push('$');
                i += 1;
                continue;
            }
        } else {
            // simple $ARGUMENTS | $@ | $DIGITS
            // Try $ARGUMENTS
            if content[i..].starts_with("$ARGUMENTS") {
                out.push_str(&all_args);
                i += "$ARGUMENTS".len();
                continue;
            }
            if next == b'@' {
                out.push_str(&all_args);
                i += 2;
                continue;
            }
            if next.is_ascii_digit() {
                // consume consecutive digits
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                let num_str = &content[i + 1..j];
                if let Ok(n) = num_str.parse::<usize>() {
                    if n >= 1 && n <= args.len() {
                        out.push_str(&args[n - 1]);
                    }
                    // if out of range, empty (like TS: args[index] ?? "")
                }
                i = j;
                continue;
            }
            // bare $ not followed by known pattern -> literal
            out.push('$');
            i += 1;
            continue;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Loader helpers
// ---------------------------------------------------------------------------

fn load_template_from_file(file_path: &Path, source: &str, base_dir: &Path) -> Option<PromptTemplate> {
    let raw = fs::read_to_string(file_path).ok()?;
    let (frontmatter, body) = parse_frontmatter(&raw);
    let name = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let mut description = frontmatter.description.unwrap_or_default();
    if description.is_empty() {
        if let Some(first) = body.lines().find(|l| !l.trim().is_empty()) {
            description = first.to_string();
            if description.len() > 60 {
                description.truncate(60);
                description.push_str("...");
            }
        }
    }
    Some(PromptTemplate {
        name,
        description,
        argument_hint: frontmatter.argument_hint,
        content: body,
        file_path: file_path.to_path_buf(),
        source: source.to_string(),
        base_dir: base_dir.to_path_buf(),
    })
}

fn load_templates_from_dir(dir: &Path, source: &str, base_dir: &Path) -> Vec<PromptTemplate> {
    let mut templates = Vec::new();
    if !dir.exists() {
        return templates;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return templates,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let is_symlink = ft.is_symlink();
        let is_file = if is_symlink {
            fs::metadata(&path).map(|m| m.is_file()).unwrap_or(false)
        } else {
            ft.is_file()
        };
        if !is_file {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Some(t) = load_template_from_file(&path, source, base_dir) {
            templates.push(t);
        }
    }
    templates
}

// ---------------------------------------------------------------------------
// Public loader — port of loadPromptTemplates
// ---------------------------------------------------------------------------

pub fn load_prompt_templates(options: LoadPromptTemplatesOptions) -> Vec<PromptTemplate> {
    let resolved_cwd = if options.cwd.as_os_str().is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        options.cwd.clone()
    };
    let resolved_agent_dir = options.agent_dir.clone().unwrap_or_else(gray_agent_dir);

    let mut templates = Vec::new();

    let global_prompts_dir = resolved_agent_dir.join("prompts");
    // project dirs: check both .pi and .gray, walking up to git root for .gray
    let git_root = find_git_root(&resolved_cwd);

    if options.include_defaults {
        // global
        templates.extend(load_templates_from_dir(&global_prompts_dir, "user", &global_prompts_dir));
        if let Some(home) = resolve_home() {
            let pi_prompts = home.join(".pi").join("agent").join("prompts");
            if pi_prompts != global_prompts_dir {
                templates.extend(load_templates_from_dir(&pi_prompts, "user", &pi_prompts));
            }
        }
        // project local — pi style single dir
        let pi_project = resolved_cwd.join(".pi").join("prompts");
        templates.extend(load_templates_from_dir(&pi_project, "project", &pi_project));
        // gray project: walk up to git root searching .gray/prompts at each ancestor
        let mut cur = Some(resolved_cwd.clone());
        let mut seen_dirs = std::collections::HashSet::new();
        while let Some(dir) = cur {
            let d = dir.join(".gray").join("prompts");
            if seen_dirs.insert(d.clone()) {
                templates.extend(load_templates_from_dir(&d, "project", &d));
            }
            if let Some(root) = &git_root {
                if &dir == root {
                    break;
                }
            }
            cur = dir.parent().map(|p| p.to_path_buf());
        }
        // also cwd/.gray/prompts if git_root not found (already covered by walk above)
    }

    // helper to classify source for explicit paths
    let is_under = |target: &Path, root: &Path| -> bool {
        if target == root {
            return true;
        }
        target.strip_prefix(root).is_ok()
    };

    for raw in &options.prompt_paths {
        let resolved = if raw.is_absolute() {
            raw.clone()
        } else {
            resolved_cwd.join(raw)
        };
        if !resolved.exists() {
            continue;
        }
        let meta = match fs::metadata(&resolved) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let source = if is_under(&resolved, &global_prompts_dir) {
            "user"
        } else {
            "path"
        };
        let base = if meta.is_dir() {
            resolved.clone()
        } else {
            resolved.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| resolved.clone())
        };
        if meta.is_dir() {
            templates.extend(load_templates_from_dir(&resolved, source, &base));
        } else if meta.is_file() && resolved.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Some(t) = load_template_from_file(&resolved, source, &base) {
                templates.push(t);
            }
        }
    }

    templates
}

/// Expand a slash-command if it matches a template name (port of expandPromptTemplate).
pub fn expand_prompt_template(text: &str, templates: &[PromptTemplate]) -> String {
    if !text.starts_with('/') {
        return text.to_string();
    }
    // match /^\/([^\s]+)(?:\s+([\s\S]*))?$/
    let trimmed = text.trim_end();
    let space_pos = trimmed.find(|c: char| c.is_whitespace());
    let (name, args_string) = if let Some(pos) = space_pos {
        (&trimmed[1..pos], trimmed[pos..].trim_start())
    } else {
        (&trimmed[1..], "")
    };
    if name.is_empty() {
        return text.to_string();
    }
    if let Some(t) = templates.iter().find(|t| t.name == name) {
        let args = parse_command_args(args_string);
        return substitute_args(&t.content, &args);
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_args_quoted() {
        assert_eq!(parse_command_args(r#"a "b c" d"#), vec!["a", "b c", "d"]);
    }
    #[test]
    fn substitute_positional() {
        assert_eq!(substitute_args("hi $1", &["world".to_string()]), "hi world");
    }
    #[test]
    fn substitute_all() {
        assert_eq!(substitute_args("$@", &["a".to_string(), "b".to_string()]), "a b");
    }
    #[test]
    fn substitute_default() {
        assert_eq!(substitute_args("${1:-fallback}", &[]), "fallback");
        assert_eq!(substitute_args("${1:-fallback}", &["x".to_string()]), "x");
    }
}
