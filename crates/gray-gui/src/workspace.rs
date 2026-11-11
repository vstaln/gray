use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
}

const SKIP: &[&str] = &[".git", "target", "node_modules", ".gray"];

pub fn tree(root: &Path, expanded: &HashSet<String>) -> Vec<FileEntry> {
    let mut out = Vec::new();
    walk(root, root, 0, expanded, &mut out);
    out
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    expanded: &HashSet<String>,
    out: &mut Vec<FileEntry>,
) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| (!e.path().is_dir(), e.file_name()));
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        if SKIP.contains(&name.as_str()) {
            continue;
        }
        let path = e.path();
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let is_dir = path.is_dir();
        out.push(FileEntry {
            rel: rel.clone(),
            name,
            depth,
            is_dir,
        });
        if is_dir && expanded.contains(&rel) {
            walk(root, &path, depth + 1, expanded, out);
        }
    }
}

pub fn all_files(root: &Path, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    collect(root, root, &mut out, limit);
    out.sort();
    out
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>, limit: usize) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for e in read.filter_map(Result::ok) {
        if out.len() >= limit {
            return;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if SKIP.contains(&name.as_str()) {
            continue;
        }
        let path = e.path();
        if path.is_dir() {
            collect(root, &path, out, limit);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().to_string());
        }
    }
}

pub fn language_for(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
    {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" => "javascript",
        "json" => "json",
        "toml" => "toml",
        "md" => "markdown",
        "py" => "python",
        "go" => "go",
        "sh" | "bash" => "bash",
        "yml" | "yaml" => "yaml",
        "html" => "html",
        "css" => "css",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" => "cpp",
        _ => "text",
    }
}

/// One event from a terminal-drawer command.
pub enum ShellMsg {
    Line(String),
    /// The shell's working directory when the command finished, so `cd`
    /// carries over to the next command.
    Cwd(std::path::PathBuf),
}

const CWD_MARK: &str = "\u{1f}GRAYCWD ";

/// Run `command` under `sh` in `cwd`. stderr is merged into stdout for the
/// whole script (a trailing `2>&1` would only cover the last command), and
/// the final `$PWD` is reported after a marker.
pub fn run_shell(cwd: &Path, command: &str) -> Receiver<ShellMsg> {
    let (tx, rx) = channel();
    let cwd = cwd.to_path_buf();
    // Newlines keep a trailing `# comment` in the command from eating the
    // wrapper.
    let script = format!(
        "exec 2>&1\n{command}\n__gray_s=$?\nprintf '\\n\\037GRAYCWD %s\\n' \"$PWD\"\nexit $__gray_s"
    );
    thread::spawn(move || {
        let child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn();
        match child {
            Ok(mut child) => {
                if let Some(out) = child.stdout.take() {
                    let mut pending: Option<String> = None;
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        if let Some(dir) = line.strip_prefix(CWD_MARK) {
                            // The marker's leading newline adds one blank
                            // line; drop it.
                            if let Some(p) = pending.take().filter(|p| !p.is_empty()) {
                                let _ = tx.send(ShellMsg::Line(p));
                            }
                            let _ = tx.send(ShellMsg::Cwd(dir.into()));
                            continue;
                        }
                        if let Some(p) = pending.replace(strip_ansi(&line)) {
                            let _ = tx.send(ShellMsg::Line(p));
                        }
                    }
                    if let Some(p) = pending {
                        let _ = tx.send(ShellMsg::Line(p));
                    }
                }
                let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
                if code != 0 {
                    let _ = tx.send(ShellMsg::Line(format!("[exit {code}]")));
                }
            }
            Err(e) => {
                let _ = tx.send(ShellMsg::Line(format!("error: {e}")));
            }
        }
    });
    rx
}

/// Drop ANSI escape sequences (CSI/OSC) and stray control bytes; the drawer
/// renders plain text.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\u{1b}' => match it.next() {
                Some('[') => {
                    for c in it.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = it.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && it.peek() == Some(&'\\') {
                            it.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push_str("    "),
            '\r' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cwd: &Path, cmd: &str) -> (Vec<String>, Option<std::path::PathBuf>) {
        let mut lines = Vec::new();
        let mut dir = None;
        for m in run_shell(cwd, cmd) {
            match m {
                ShellMsg::Line(l) => lines.push(l),
                ShellMsg::Cwd(d) => dir = Some(d),
            }
        }
        (lines, dir)
    }

    #[test]
    fn stderr_of_every_command_is_captured() {
        let (lines, _) = run(Path::new("/"), "echo out; echo err >&2; false");
        assert_eq!(lines, ["out", "err", "[exit 1]"]);
    }

    #[test]
    fn cd_is_reported_and_no_blank_tail() {
        let (lines, dir) = run(Path::new("/"), "cd /tmp # comment");
        assert!(lines.is_empty(), "{lines:?}");
        assert_eq!(dir.unwrap(), Path::new("/tmp"));
    }

    #[test]
    fn output_without_trailing_newline_survives() {
        let (lines, _) = run(Path::new("/"), "printf abc");
        assert_eq!(lines, ["abc"]);
    }

    #[test]
    fn ansi_is_stripped() {
        assert_eq!(strip_ansi("\u{1b}[01;34mdir\u{1b}[0m x"), "dir x");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}hi"), "hi");
    }
}
