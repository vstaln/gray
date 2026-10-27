use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Note,

    Caution,

    Critical,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Note => "note",
            Severity::Caution => "caution",
            Severity::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub rule: &'static str,
    pub severity: Severity,

    pub path: String,

    pub line: usize,

    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Safe,
    Caution,
    Dangerous,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Safe => "safe",
            Verdict::Caution => "caution",
            Verdict::Dangerous => "dangerous",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub findings: Vec<Finding>,

    pub files: usize,

    pub skipped: usize,
}

impl Report {
    pub fn verdict(&self) -> Verdict {
        if self
            .findings
            .iter()
            .any(|f| f.severity == Severity::Critical)
        {
            Verdict::Dangerous
        } else if self
            .findings
            .iter()
            .any(|f| f.severity == Severity::Caution)
        {
            Verdict::Caution
        } else {
            Verdict::Safe
        }
    }

    pub fn criticals(&self) -> Vec<&Finding> {
        let mut out: Vec<&Finding> = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Critical)
            .collect();
        out.sort_by_key(|f| (f.rule, f.path.clone(), f.line));
        out
    }

    pub fn summary(&self) -> String {
        let critical = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Critical)
            .count();
        let caution = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Caution)
            .count();
        let note = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Note)
            .count();
        if self.findings.is_empty() {
            return format!("{} files scanned, no findings", self.files);
        }
        format!(
            "{} critical, {} caution, {} note ({} findings in {} files)",
            critical,
            caution,
            note,
            self.findings.len(),
            self.files
        )
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for f in &self.findings {
            out.push_str(&format!(
                "  [{}] {}:{} — {}\n    {}\n",
                f.severity.label(),
                f.path,
                f.line,
                f.rule,
                f.detail
            ));
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Prose,

    Test,

    Code,
}

fn is_prose(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some("md" | "markdown" | "txt" | "rst" | "html" | "htm" | "adoc") => true,
        _ => {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            matches!(
                name.as_str(),
                "readme" | "changelog" | "license" | "notice" | "authors" | "contributing"
            )
        }
    }
}

fn is_test(path: &Path) -> bool {
    const DIRS: &[&str] = &[
        "tests",
        "test",
        "testing",
        "spec",
        "specs",
        "fixtures",
        "testdata",
        "benches",
        "examples",
        "__tests__",
        "__fixtures__",
    ];
    if path
        .components()
        .any(|c| DIRS.contains(&c.as_os_str().to_str().unwrap_or_default()))
    {
        return true;
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    name.ends_with("_tests.rs")
        || name.ends_with("_test.rs")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.starts_with("test_")
}

fn under_skills_dir(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str().to_str() == Some("skills"))
}

fn is_script(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("sh" | "bash" | "zsh" | "fish" | "py" | "pl" | "rb" | "ps1" | "bat" | "cmd" | "lua")
    )
}

fn line_executes(line: &str) -> bool {
    const EXECUTORS: &[&str] = &[
        "system(",
        "os.system",
        "popen",
        "subprocess",
        "exec(",
        "execfile",
        "spawn",
        "command::new",
        "process::command",
        "sh -c",
        "bash -c",
        "-c \"",
        "-c '",
        "eval(",
        "child_process",
        "runtime.exec",
        "child_process",
    ];
    let h = line.to_ascii_lowercase();
    EXECUTORS.iter().any(|e| h.contains(e))
}

fn classify(path: &Path) -> Class {
    if is_test(path) {
        return Class::Test;
    }
    if is_prose(path) && !under_skills_dir(path) {
        return Class::Prose;
    }
    Class::Code
}

fn is_vendored(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some(".git")
                | Some("target")
                | Some("node_modules")
                | Some(".svn")
                | Some("__pycache__")
        )
    })
}

const MAX_FILE_BYTES: u64 = 1024 * 1024;

const MAX_PER_RULE_PER_FILE: usize = 5;

const MAX_FINDINGS: usize = 300;

fn rm_invocation(line: &str) -> Option<(String, String)> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    for (i, tok) in tokens.iter().enumerate() {
        let base = tok
            .rsplit(['/', '\\', '"', '\'', '=', '(', '`', ',', ';'])
            .next()
            .unwrap_or(tok)
            .trim_matches(|c: char| c == '"' || c == '\'');
        if base != "rm" {
            continue;
        }
        let mut flags = String::new();
        for raw in &tokens[i + 1..] {
            let t =
                raw.trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | ',' | ';' | '`'));
            if t == "--" {
                continue;
            }
            if t.starts_with('-') && !t.starts_with("--") && flags.is_empty() {
                flags = t.to_string();
                continue;
            }
            return Some((flags, t.to_string()));
        }
    }
    None
}

fn rm_recursive_force(line: &str) -> bool {
    rm_invocation(line).is_some_and(|(f, _)| f.contains('r') && f.contains('f'))
}

fn destructive_root_rm(line: &str) -> bool {
    rm_invocation(line).is_some_and(|(f, target)| {
        f.contains('r') && f.contains('f') && matches!(target.as_str(), "/" | "/*")
    })
}

fn self_uninstall(line: &str) -> bool {
    let hay = line.to_ascii_lowercase();
    rm_recursive_force(line)
        && (hay.contains(".gray/plugins") || hay.contains("~/.gray") || hay.contains("$home/.gray"))
}

fn reverse_shell(line: &str) -> bool {
    let h = line.to_ascii_lowercase();
    h.contains("/dev/tcp/")
        || h.contains("/dev/udp/")
        || h.contains("nc -e")
        || h.contains("ncat -e")
        || h.contains("nc.traditional.c -e")
}

fn curl_pipe_shell(line: &str) -> bool {
    let h = line.to_ascii_lowercase();
    let fetches = |c: &str| h.contains(&format!("{c} ")) || h.contains(&format!("{c}\t"));
    (fetches("curl") || fetches("wget") || fetches("fetch"))
        && (h.contains("| sh")
            || h.contains("|sh")
            || h.contains("| bash")
            || h.contains("|bash")
            || h.contains("| python")
            || h.contains("|python")
            || h.contains("| zsh")
            || h.contains("| perl")
            || h.contains("| node")
            || h.contains("|node"))
}

fn authorized_keys_append(line: &str) -> bool {
    let h = line.to_ascii_lowercase();
    (h.contains(">>") || h.contains(" tee ")) && (h.contains("authorized_keys"))
}

fn obfuscated_exec(line: &str) -> bool {
    let h = line.to_ascii_lowercase();
    (h.contains("base64 -d") || h.contains("base64 --decode") || h.contains("base64.b64decode"))
        && (h.contains("| sh")
            || h.contains("|sh")
            || h.contains("| bash")
            || h.contains("|bash")
            || h.contains("| python")
            || h.contains("|python")
            || h.contains("| perl")
            || h.contains("| node")
            || h.contains("|node")
            || h.contains("eval(")
            || h.contains("exec("))
}

fn base64_text_filter(line: &str) -> bool {
    let h = line.to_ascii_lowercase();
    (h.contains("base64 -d") || h.contains("base64 --decode"))
        && (h.contains("grep") || h.contains("jq") || h.contains("sed"))
}

fn base64_media(line: &str) -> bool {
    let h = line.to_ascii_lowercase();
    h.contains("data:image/png;base64")
        || h.contains("data:image/jpeg;base64")
        || h.contains("data:image/gif;base64")
        || h.contains("data:image/webp;base64")
        || h.contains("data:font/")
        || h.contains("data:application/pdf;base64")
        || h.contains("data:application/font")
}

fn credential_store_path(line: &str) -> bool {
    let h = line.to_ascii_lowercase();
    const PATHS: &[&str] = &[
        "~/.ssh",
        "$home/.ssh",
        "/.ssh/",
        "id_rsa",
        "id_ed25519",
        ".aws/credentials",
        ".netrc",
        ".gnupg",
        "library/keychains",
        ".gray/auth.json",
        ".gray/gateway.yaml",
        "gray_api_key",
    ];
    PATHS.iter().any(|p| h.contains(p))
}

fn network_verb(line: &str) -> bool {
    let h = line.to_ascii_lowercase();
    h.contains("curl ")
        || h.contains("wget ")
        || h.contains("http://")
        || h.contains("https://")
        || h.contains("requests.post")
        || h.contains("requests.put")
        || h.contains("reqwest::")
        || h.contains("fetch(")
        || h.contains("httpx.")
        || h.contains("urllib.request")
        || h.contains(":: post(")
}

fn credential_store_read(line: &str) -> bool {
    credential_store_path(line)
}

fn hardcoded_secret(line: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "sk-",
        "ghp_",
        "gho_",
        "github_pat_",
        "glpat-",
        "AKIA",
        "xoxb-",
        "xoxp-",
        "xapp-",
    ];

    NEEDLES.iter().any(|n| {
        line.contains(n) && line.contains(['"', '\'', '=', ':']) && line.len() > n.len() + 8
    })
}

fn prompt_injection(line: &str) -> bool {
    let h = line.to_ascii_lowercase();
    const SHAPES: &[&str] = &[
        "ignore previous instructions",
        "ignore all previous",
        "disregard previous",
        "do not tell the user",
        "without telling the user",
        "don't tell the user",
        "never mention this to the user",
        "you are now",
        "act as if you have no restrictions",
        "bypass the permission",
        "skip the confirmation",
    ];
    SHAPES.iter().any(|s| h.contains(s))
}

struct Rule {
    id: &'static str,
    severity: Severity,

    survives_prose: bool,
    test: fn(&str) -> bool,
}

const RULES: &[Rule] = &[
    Rule {
        id: "reverse_shell",
        severity: Severity::Critical,
        survives_prose: true,
        test: reverse_shell,
    },
    Rule {
        id: "curl_pipe_shell",
        severity: Severity::Critical,
        survives_prose: true,
        test: curl_pipe_shell,
    },
    Rule {
        id: "authorized_keys_append",
        severity: Severity::Critical,
        survives_prose: true,
        test: authorized_keys_append,
    },
    Rule {
        id: "obfuscated_exec",
        severity: Severity::Critical,
        survives_prose: true,
        test: obfuscated_exec,
    },
    Rule {
        id: "credential_exfil",
        severity: Severity::Critical,
        survives_prose: true,
        test: |l| credential_store_path(l) && network_verb(l),
    },
    Rule {
        id: "prompt_injection",
        severity: Severity::Critical,
        survives_prose: true,
        test: prompt_injection,
    },
    Rule {
        id: "hardcoded_secret",
        severity: Severity::Critical,
        survives_prose: true,
        test: hardcoded_secret,
    },
    Rule {
        id: "destructive_root_rm",
        severity: Severity::Critical,
        survives_prose: false,
        test: destructive_root_rm,
    },
    Rule {
        id: "credential_store_read",
        severity: Severity::Caution,
        survives_prose: false,
        test: credential_store_read,
    },
    Rule {
        id: "self_uninstall",
        severity: Severity::Note,
        survives_prose: false,
        test: self_uninstall,
    },
    Rule {
        id: "base64_text_filter",
        severity: Severity::Note,
        survives_prose: false,
        test: base64_text_filter,
    },
    Rule {
        id: "base64_media",
        severity: Severity::Note,
        survives_prose: false,
        test: base64_media,
    },
];

fn adjust(rule: &Rule, class: Class, rel: &str, line: &str) -> Severity {
    match class {
        Class::Code => rule.severity,
        Class::Prose => {
            if rule.survives_prose {
                rule.severity
            } else {
                match rule.severity {
                    Severity::Critical => Severity::Caution,
                    Severity::Caution => Severity::Note,
                    Severity::Note => Severity::Note,
                }
            }
        }
        Class::Test => match rule.severity {
            Severity::Critical if rule.id == "hardcoded_secret" => Severity::Note,
            Severity::Critical => {
                if is_script(Path::new(rel)) || line_executes(line) {
                    Severity::Caution
                } else {
                    Severity::Note
                }
            }
            other => other,
        },
    }
}

pub fn scan_tree(root: &Path) -> anyhow::Result<Report> {
    let mut report = Report::default();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut children: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();

        children.sort();
        for path in children {
            if is_vendored(&path) {
                report.skipped += 1;
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if report.findings.len() >= MAX_FINDINGS {
                break;
            }
            scan_file(root, &path, &mut report);
        }
    }
    Ok(report)
}

fn scan_file(root: &Path, path: &Path, report: &mut Report) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        report.skipped += 1;
        return;
    }
    let Ok(bytes) = std::fs::read(path) else {
        report.skipped += 1;
        return;
    };

    let probe = &bytes[..bytes.len().min(8192)];
    if probe.contains(&0) {
        report.skipped += 1;
        return;
    }
    let text = String::from_utf8_lossy(&bytes);
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let class = classify(Path::new(&rel));
    report.files += 1;

    let mut per_rule: std::collections::HashMap<&'static str, usize> =
        std::collections::HashMap::new();
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        for rule in RULES {
            if !(rule.test)(trimmed) {
                continue;
            }
            let seen = per_rule.entry(rule.id).or_insert(0);
            if *seen >= MAX_PER_RULE_PER_FILE {
                continue;
            }
            *seen += 1;
            report.findings.push(Finding {
                rule: rule.id,
                severity: adjust(rule, class, &rel, trimmed),
                path: rel.clone(),
                line: idx + 1,
                detail: truncate(trimmed),
            });
        }
    }
}

fn truncate(s: &str) -> String {
    const MAX: usize = 160;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(MAX).collect();
        out.push('…');
        out
    }
}
