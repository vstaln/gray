//! The `find` tool: glob filename search. Respects .gitignore.

use std::path::Path;

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::io::AsyncBufReadExt;

use crate::{fail, finish, get_opt_u64, get_str, resolve_path, Tool, MAX_BYTES};

const DEFAULT_LIMIT: usize = 1000;

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn truncate_head(content: &str, max_bytes: usize) -> (String, bool) {
    if content.len() <= max_bytes {
        return (content.to_string(), false);
    }
    let mut out_lines: Vec<&str> = Vec::new();
    let mut bytes = 0usize;
    for (i, line) in content.split('\n').enumerate() {
        let line_bytes = line.len() + if i > 0 { 1 } else { 0 };
        if bytes + line_bytes > max_bytes {
            break;
        }
        out_lines.push(line);
        bytes += line_bytes;
    }
    if out_lines.is_empty() {
        return (String::new(), true);
    }
    (out_lines.join("\n"), true)
}

fn relativize(result_path: &str, search_path: &Path) -> String {
    let rp = Path::new(result_path);
    let had_trailing_sep = result_path.ends_with('/') || result_path.ends_with(std::path::MAIN_SEPARATOR);
    let relative = if rp.is_absolute() {
        rp.strip_prefix(search_path).unwrap_or(rp).to_path_buf()
    } else {
        rp.to_path_buf()
    };
    let mut posix = relative.to_string_lossy().replace('\\', "/");
    if had_trailing_sep && !posix.ends_with('/') && !posix.is_empty() {
        posix.push('/');
    }
    if posix.is_empty() {
        result_path.to_string()
    } else {
        posix
    }
}

pub const FIND_SNIPPET: &str = "Find files by glob pattern (respects .gitignore)";
pub const FIND_GUIDELINES: &[&str] = &[];

/// Filename glob search. Respects .gitignore via `fd` when available,
/// otherwise falls back to a manual walk.
pub struct FindTool;

#[async_trait]
impl Tool for FindTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "find",
            format!(
                "Search for files by glob pattern. Returns matching file paths relative to the search directory. Respects .gitignore. Output is truncated to {DEFAULT_LIMIT} results or {}KB (whichever is hit first).",
                MAX_BYTES / 1024
            ),
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'" },
                    "path": { "type": "string", "description": "Directory to search in (default: current directory)" },
                    "limit": { "type": "integer", "description": "Maximum number of results (default: 1000)" }
                },
                "required": ["pattern"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&'static str> {
        Some(FIND_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(FIND_GUIDELINES)
    }

    fn is_concurrency_safe(&self, _args: &Value) -> bool {
        true
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let pattern = match get_str(&args, "pattern") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let search_dir = match args.get("path") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return fail("invalid argument 'path': expected string".to_string()),
        };
        let effective_limit = match get_opt_u64(&args, "limit") {
            Ok(v) => v.map(|n| n as usize).unwrap_or(DEFAULT_LIMIT).max(1),
            Err(e) => return e,
        };

        let search_path = resolve_path(&ctx.cwd, search_dir.as_deref().unwrap_or("."));

        match tokio::fs::metadata(&search_path).await {
            Ok(m) if m.is_dir() => {},
            Ok(_) => return fail(format!("Not a directory: {}", search_path.display())),
            Err(e) => return fail(format!("Path not found: {}: {e}", search_path.display())),
        }

        // Try fd first (preferred, respects .gitignore correctly).
        if let Some(output) = try_fd(&pattern, &search_path, effective_limit).await {
            return output;
        }

        // Fallback: manual recursive walk with simple glob matching.
        fallback_walk(&pattern, &search_path, effective_limit).await
    }
}

async fn try_fd(pattern: &str, search_path: &Path, effective_limit: usize) -> Option<ToolOutput> {
    // Probe fd availability quickly.
    let mut args: Vec<String> = vec!["--glob".to_string(), "--color=never".to_string(), "--hidden".to_string()];

    // Detect git repo to decide --no-require-git
    let mut inside_git = false;
    let mut cur = search_path.to_path_buf();
    loop {
        if tokio::fs::metadata(cur.join(".git")).await.is_ok() {
            inside_git = true;
            break;
        }
        match cur.parent() {
            Some(p) if p != cur => cur = p.to_path_buf(),
            _ => break,
        }
    }
    if !inside_git {
        args.push("--no-require-git".to_string());
    }
    args.push("--max-results".to_string());
    args.push(effective_limit.to_string());

    let mut effective_pattern = pattern.to_string();
    let needs_full_path = pattern.contains('/');
    if needs_full_path {
        args.push("--full-path".to_string());
        if !pattern.starts_with('/') && !pattern.starts_with("**/") && pattern != "**" {
            effective_pattern = format!("**/{pattern}");
        }
        #[cfg(windows)]
        {
            effective_pattern = effective_pattern.replace('/', "[/\\\\]");
        }
    }
    args.push("--".to_string());
    args.push(effective_pattern);
    args.push(search_path.to_string_lossy().to_string());

    let mut child = match Command::new("fd")
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return None,
    };

    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let stderr_handle = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let mut r = stderr;
        let mut tmp = [0u8; 1024];
        loop {
            match r.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    });

    let mut reader = tokio::io::BufReader::new(stdout).lines();
    let mut lines: Vec<String> = Vec::new();
    while let Ok(Some(line)) = reader.next_line().await {
        lines.push(line);
    }
    let status = child.wait().await.ok()?;
    let stderr_str = stderr_handle.await.unwrap_or_default();

    // If fd exited with error and produced no output, treat as failure and fall back.
    if let Some(code) = status.code() {
        if code != 0 && lines.is_empty() {
            // Check if fd is actually usable; if error is about missing fd, fall back.
            // Otherwise surface the error.
            let msg = stderr_str.trim();
            if !msg.is_empty() && lines.is_empty() {
                // If no output, let fallback handle it or return no matches.
                // Only return error if we clearly have no results and an error.
                // For now, fall through to fallback if we got nothing.
                if code != 0 && code != 1 {
                    // Return error only if we have stderr and no fallback would help.
                    // But still try fallback first by returning None? Let's surface error.
                    return Some(fail(msg.to_string()));
                }
            }
        }
    }

    if lines.is_empty() {
        return Some(finish("No files found matching pattern".to_string()));
    }

    let relativized: Vec<String> = lines
        .iter()
        .map(|l| {
            let trimmed = l.trim().trim_end_matches('\r').to_string();
            if trimmed.is_empty() {
                return String::new();
            }
            relativize(&trimmed, search_path)
        })
        .filter(|s| !s.is_empty())
        .collect();

    if relativized.is_empty() {
        return Some(finish("No files found matching pattern".to_string()));
    }

    let result_limit_reached = relativized.len() >= effective_limit;
    let raw_output = relativized.join("\n");
    let (mut output, byte_truncated) = truncate_head(&raw_output, MAX_BYTES);

    let mut notices: Vec<String> = Vec::new();
    if result_limit_reached {
        notices.push(format!(
            "{effective_limit} results limit reached. Use limit={} for more, or refine pattern",
            effective_limit * 2
        ));
    }
    if byte_truncated {
        notices.push(format!("{} limit reached", format_size(MAX_BYTES)));
    }
    if !notices.is_empty() {
        output.push_str("\n\n[");
        output.push_str(&notices.join(". "));
        output.push(']');
    }

    Some(finish(output))
}

async fn fallback_walk(pattern: &str, search_path: &Path, effective_limit: usize) -> ToolOutput {
    // Use glob crate-style matching via simple conversion to a matcher.
    // We support `*`, `**`, `?`, and `{a,b}`-like? For now handle `*` and `**`.
    // For correctness we try to use the `glob` pattern via matching file paths.
    // We'll read .gitignore if present and skip matching ignores.

    let gitignore_patterns = read_gitignore(search_path).await;

    let mut results: Vec<String> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![search_path.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy().to_string();

            // Skip .git and node_modules always.
            if name == ".git" || name == "node_modules" {
                continue;
            }
            // Check .gitignore (simple prefix/glob check)
            if is_ignored(&path, search_path, &gitignore_patterns) {
                continue;
            }

            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                stack.push(path.clone());
            }

            // Match glob against relative path and basename.
            let rel = path.strip_prefix(search_path).unwrap_or(&path);
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let basename = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();

            if glob_matches(pattern, &rel_str, &basename) {
                // For directories, include trailing slash like pi does? For find we return files; include dirs with slash.
                let mut out = rel_str.clone();
                if is_dir && !out.ends_with('/') {
                    out.push('/');
                }
                results.push(out);
                if results.len() >= effective_limit {
                    break;
                }
            }
        }
        if results.len() >= effective_limit {
            break;
        }
    }

    if results.is_empty() {
        return finish("No files found matching pattern".to_string());
    }

    results.sort();
    let result_limit_reached = results.len() >= effective_limit;
    let raw_output = results.join("\n");
    let (mut output, byte_truncated) = truncate_head(&raw_output, MAX_BYTES);

    let mut notices: Vec<String> = Vec::new();
    if result_limit_reached {
        notices.push(format!(
            "{effective_limit} results limit reached. Use limit={} for more, or refine pattern",
            effective_limit * 2
        ));
    }
    if byte_truncated {
        notices.push(format!("{} limit reached", format_size(MAX_BYTES)));
    }
    if !notices.is_empty() {
        output.push_str("\n\n[");
        output.push_str(&notices.join(". "));
        output.push(']');
    }

    finish(output)
}

async fn read_gitignore(search_path: &Path) -> Vec<String> {
    let mut patterns = Vec::new();
    // Walk up to find .gitignore at search_path only (simple).
    let gi = search_path.join(".gitignore");
    if let Ok(content) = tokio::fs::read_to_string(&gi).await {
        for line in content.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            patterns.push(l.to_string());
        }
    }
    patterns
}

fn is_ignored(path: &Path, search_path: &Path, patterns: &[String]) -> bool {
    let rel = path.strip_prefix(search_path).unwrap_or(path);
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    for pat in patterns {
        // Very simple: if pattern is a prefix or matches basename
        let p = pat.trim_end_matches('/');
        if p.contains('*') || p.contains('?') {
            if glob_matches(p, &rel_str, &rel_str) {
                return true;
            }
        } else if rel_str == *p || rel_str.starts_with(&format!("{p}/")) {
            return true;
        }
    }
    false
}

fn glob_matches(pattern: &str, rel_path: &str, basename: &str) -> bool {
    // Use a lightweight glob matcher without external crates.
    // Supports: *, **, ?, and literal segments.
    // For patterns without '/', match against basename only (like fd default).
    // For patterns with '/', match against rel_path.
    let target = if pattern.contains('/') { rel_path } else { basename };
    matches_glob(pattern, target)
}

fn matches_glob(pattern: &str, text: &str) -> bool {
    // Convert glob to a simple recursive matcher.
    // Split pattern by '/' for ** handling, but for non-** we treat as single string glob.
    if pattern.contains("**") {
        return matches_with_doublestar(pattern, text);
    }
    matches_star(pattern, text)
}

fn matches_with_doublestar(pattern: &str, text: &str) -> bool {
    // Split on "**" and require each segment to appear in order.
    let parts: Vec<&str> = pattern.split("**").collect();
    if parts.is_empty() {
        return true;
    }
    // parts[0] must be prefix, parts[last] must be suffix, middle parts anywhere in order.
    let mut remaining = text;

    // Prefix
    if !parts[0].is_empty() {
        let prefix = parts[0].trim_matches('/');
        if !prefix.is_empty() {
            // prefix may contain * — match against start of remaining
            // Find the prefix match at start
            if !matches_prefix(prefix, remaining) {
                return false;
            }
            // Advance past the matched prefix segment length (approx).
            // Instead, try to find where prefix glob could end: search for next '/' boundary.
            // Simpler: check if remaining starts with a path that matches prefix glob up to next '/'.
            // We'll use a sliding window: try every split point.
            let mut found = false;
            for i in 0..=remaining.len() {
                if remaining.is_char_boundary(i) {
                    let prefix_slice = &remaining[..i];
                    if matches_star(prefix, prefix_slice) {
                        remaining = &remaining[i..];
                        // consume optional '/'
                        if remaining.starts_with('/') {
                            remaining = &remaining[1..];
                        }
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                return false;
            }
        } else if remaining.starts_with('/') {
            remaining = &remaining[1..];
        }
    }

    for (idx, part) in parts[1..].iter().enumerate() {
        let is_last = idx == parts[1..].len() - 1;
        let seg = part.trim_matches('/');
        if seg.is_empty() {
            continue;
        }
        if is_last {
            // suffix must match end
            // Try every position from start to end
            let mut matched = false;
            for i in 0..=remaining.len() {
                if remaining.is_char_boundary(i) && matches_star(seg, &remaining[i..]) {
                    matched = true;
                    break;
                }
                // For suffix that should match end, also try matching suffix of remaining
                if remaining[i..].len() <= seg.len() + 20 {
                    // not needed
                }
            }
            // More precise: does the tail of remaining match seg?
            // Check if any suffix of remaining matches seg
            let mut ok = false;
            for i in 0..=remaining.len() {
                if remaining.is_char_boundary(i) && matches_star(seg, &remaining[i..]) {
                    ok = true;
                }
            }
            if !ok && !matched {
                return false;
            }
            return ok || matched;
        } else {
            // middle part must appear somewhere
            let mut found = false;
            for i in 0..=remaining.len() {
                if remaining.is_char_boundary(i) {
                    // try to match seg at position i
                    for j in i..=remaining.len() {
                        if remaining.is_char_boundary(j) && matches_star(seg, &remaining[i..j]) {
                            remaining = &remaining[j..];
                            if remaining.starts_with('/') {
                                remaining = &remaining[1..];
                            }
                            found = true;
                            break;
                        }
                    }
                    if found {
                        break;
                    }
                }
            }
            if !found {
                return false;
            }
        }
    }
    true
}

fn matches_prefix(pattern: &str, text: &str) -> bool {
    // Does text start with something matching pattern?
    for i in 0..=text.len() {
        if text.is_char_boundary(i) && matches_star(pattern, &text[..i]) {
            return true;
        }
    }
    false
}

fn matches_star(pattern: &str, text: &str) -> bool {
    // Classic wildcard matching for * and ?
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut match_idx) = (None::<usize>, 0usize);

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            match_idx = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            match_idx += 1;
            ti = match_idx;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}
