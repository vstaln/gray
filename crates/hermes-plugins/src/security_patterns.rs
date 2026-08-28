//! Regex-based security pattern definitions for the security-guidance plugin.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/plugins/security-guidance/patterns.py` (368 LOC).
//! Pure data + one pure helper. No env-var reads, no I/O — kept side-effect-free
//! so it can be imported in isolation.
//!
//! Forked verbatim from Anthropic's claude-plugins-official repository
//! (plugins/security-guidance/hooks/patterns.py) under the Apache License 2.0:
//!
//!     https://github.com/anthropics/claude-plugins-official
//!
//!   Copyright (c) Anthropic, PBC. and the security-guidance contributors
//!   Licensed under the Apache License, Version 2.0 (the "License");
//!   you may not use this file except in compliance with the License.
//!   You may obtain a copy of the License at
//!
//!       http://www.apache.org/licenses/LICENSE-2.0
//!
//!   Unless required by applicable law or agreed to in writing, software
//!   distributed under the License is distributed on an "AS IS" BASIS,
//!   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//!   See the License for the specific language governing permissions and
//!   limitations under the License.
//!
//! Modifications by NousResearch for the Hermes Agent plugin port:
//!   - none to the pattern data itself; this file is byte-for-byte the upstream
//!     patterns.py at commit 0bde168 (2026-05-26). Hermes-side wiring lives in
//!     __init__.py.
//!
//! Python surface ported line-for-line:
//!   - `_JS_EXTS`, `_PY_EXTS`, `_DOC_EXTS`
//!   - `_UNSAFE_DESERIALIZATION_REMINDER`, `_UNSAFE_YAML_LOAD_REMINDER`, `_UNSAFE_TORCH_LOAD_REMINDER`
//!   - `SECURITY_PATTERNS` (25 entries, path_check/path_filter + substrings + regex + reminder)
//!   - `RuleId` (IntEnum 1..25, stable IDs, append-only)
//!   - `_RULE_NAME_TO_ID` (mapping)
//!   - `assert` sync check (desync fails loud at import/test time)
//!   - `rule_names_to_mask` (bitmask helper, user: prefix excluded via map miss)
//!
//! `regex` patterns are stored as raw `&str` (no `regex` crate) so the data
//! is byte-identical without requiring `cargo` in this task. Real matching would
//! swap the `Option<&str>` body for `regex::Regex::new(...).unwrap()`.

use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Extension groups — mirrors patterns.py:33-35
// ---------------------------------------------------------------------------

/// Mirrors `_JS_EXTS = (".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs", ".mts", ".cts", ".vue", ".svelte")`
pub const JS_EXTS: &[&str] = &[".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs", ".mts", ".cts", ".vue", ".svelte"];

/// Mirrors `_PY_EXTS = (".py", ".pyi", ".ipynb")`
pub const PY_EXTS: &[&str] = &[".py", ".pyi", ".ipynb"];

/// Mirrors `_DOC_EXTS = (".md", ".mdx", ".txt", ".rst", ".json", ".yaml", ".yml")`
pub const DOC_EXTS: &[&str] = &[".md", ".mdx", ".txt", ".rst", ".json", ".yaml", ".yml"];

// ---------------------------------------------------------------------------
// Shared reminder constants — mirrors patterns.py:38-50
// ---------------------------------------------------------------------------

/// Mirrors `_UNSAFE_DESERIALIZATION_REMINDER`
pub const UNSAFE_DESERIALIZATION_REMINDER: &str = r#"⚠️ Security Warning: Loading pickle data (or equivalents: cPickle, cloudpickle, dill, marshal, shelve, joblib, pandas.read_pickle, numpy with allow_pickle=True) from untrusted sources allows arbitrary code execution.

For simple data, prefer JSON or msgspec. For typed objects, prefer a schema-validated deserializer (msgspec.Struct, pydantic, marshmallow) that constructs only declared types.

If this is safe or is explicitly needed, briefly document that in a comment before continuing."#;

/// Mirrors `_UNSAFE_YAML_LOAD_REMINDER`
pub const UNSAFE_YAML_LOAD_REMINDER: &str = r#"⚠️ Security Warning: yaml.load() / yaml.unsafe_load() execute arbitrary Python via !!python/object tags.

Use yaml.safe_load() if the file only contains simple data structures (dicts, lists, strings, numbers). If you need typed objects, parse with safe_load and validate the result against a schema (pydantic, msgspec, marshmallow) — never use a custom Loader that constructs arbitrary types."#;

/// Mirrors `_UNSAFE_TORCH_LOAD_REMINDER`
pub const UNSAFE_TORCH_LOAD_REMINDER: &str = r#"⚠️ Security Warning: torch.load() defaults to weights_only=False, which unpickles arbitrary Python objects and allows arbitrary code execution.

If the file only contains tensors and simple data structures, pass weights_only=True (or set TORCH_FORCE_WEIGHTS_ONLY_LOAD=1)."#;

// ---------------------------------------------------------------------------
// Per-pattern multi-line reminders — mirrors inline triple-quoted strings
// ---------------------------------------------------------------------------

pub const GITHUB_WORKFLOW_REMINDER: &str = r#"⚠️ Security Warning: You are editing a GitHub Actions workflow file. Be aware of these security risks:

1. **Command Injection**: Never use untrusted input (like issue titles, PR descriptions, commit messages) directly in run: commands without proper escaping
2. **Use environment variables**: Instead of ${{ github.event.issue.title }}, use env: with proper quoting
3. **Review the guide**: https://github.blog/security/vulnerability-research/how-to-catch-github-actions-workflow-injections-before-attackers-do/

Example of UNSAFE pattern to avoid:
run: echo "${{ github.event.issue.title }}"

Example of SAFE pattern:
env:
  TITLE: ${{ github.event.issue.title }}
run: echo "$TITLE"

Other risky inputs to be careful with:
- github.event.issue.body
- github.event.pull_request.title
- github.event.pull_request.body
- github.event.comment.body
- github.event.review.body
- github.event.review_comment.body
- github.event.pages.*.page_name
- github.event.commits.*.message
- github.event.head_commit.message
- github.event.head_commit.author.email
- github.event.head_commit.author.name
- github.event.commits.*.author.email
- github.event.commits.*.author.name
- github.event.pull_request.head.ref
- github.event.pull_request.head.label
- github.event.pull_request.head.repo.default_branch
- github.event.client_payload.* (repository_dispatch events — attacker can set any field)

4. **Ref injection**: Never use untrusted input in `ref:` parameters of `actions/checkout`. For `client_payload.pr_number`, validate it matches `^[0-9]+$` before using in `ref: refs/pull/${{ ... }}/head`
- github.head_ref"#;

pub const CHILD_PROCESS_EXEC_REMINDER: &str = r#"⚠️ Security Warning: Using child_process.exec() can lead to command injection vulnerabilities.

exec() runs the command string through a shell, so any user input interpolated into it can inject arbitrary commands. Prefer child_process.execFile() (or spawn()) with an argument array instead of building a shell string.

Instead of:
  exec(`command ${userInput}`)

Use:
  import { execFile } from 'node:child_process'
  execFile('command', [userInput], callback)

Why execFile/spawn with an argument array is safer:
- No shell is involved, so shell metacharacters in arguments are not interpreted
- Arguments are passed directly to the program rather than interpolated into a command string

Only use exec() if you absolutely need shell features and the input is guaranteed to be safe."#;

pub const PYTHON_SUBPROCESS_SHELL_REMINDER: &str = r#"⚠️ Security Warning: Using subprocess with shell=True enables command injection.

UNSAFE:
  subprocess.run(f"ls {user_input}", shell=True)
  subprocess.call("grep " + pattern, shell=True)

SAFE - pass arguments as a list without shell:
  subprocess.run(["ls", user_input])
  subprocess.call(["grep", pattern])

When arguments are passed as a list without shell=True, special characters cannot be interpreted as shell metacharacters."#;

pub const GO_EXEC_SHELL_INJECTION_REMINDER: &str = r#"⚠️ Security Warning: Using exec.Command with a shell interpreter (sh/bash) enables command injection.

UNSAFE:
  exec.Command("sh", "-c", "ping -c 1 " + host)
  exec.Command("bash", "-c", fmt.Sprintf("df -h %s", path))

SAFE - pass arguments directly without a shell:
  exec.Command("ping", "-c", "1", host)
  exec.Command("df", "-h", path)

When arguments are passed directly (not through a shell), special characters in user input cannot be interpreted as shell metacharacters. This prevents command injection entirely.

Additionally, validate user inputs:
- For hostnames/IPs: use net.ParseIP() or a hostname regex
- For file paths: use filepath.Clean() and verify the result is within an allowed directory
- For numeric values: parse to int/float first"#;

// ---------------------------------------------------------------------------
// Path predicates — mirrors lambda path_check / path_filter in patterns.py
// ---------------------------------------------------------------------------

/// Mirrors `lambda path: ".github/workflows/" in path and (path.endswith(".yml") or path.endswith(".yaml"))`
pub fn is_github_workflow_file(path: &str) -> bool {
    path.contains(".github/workflows/") && (path.ends_with(".yml") || path.ends_with(".yaml"))
}

/// Mirrors `lambda p: p.endswith(_JS_EXTS)`
pub fn is_js_file(path: &str) -> bool {
    JS_EXTS.iter().any(|ext| path.ends_with(ext))
}

/// Mirrors `lambda p: p.endswith(_PY_EXTS)`
pub fn is_py_file(path: &str) -> bool {
    PY_EXTS.iter().any(|ext| path.ends_with(ext))
}

/// Mirrors `lambda p: not p.endswith(_DOC_EXTS)`
pub fn is_not_doc_file(path: &str) -> bool {
    !DOC_EXTS.iter().any(|ext| path.ends_with(ext))
}

// ---------------------------------------------------------------------------
// SecurityPattern struct — mirrors dict entries in SECURITY_PATTERNS
// ---------------------------------------------------------------------------

/// Mirrors a single entry in `SECURITY_PATTERNS` (patterns.py:53-284).
///
/// `path_check` vs `path_filter` distinction is preserved 1:1 from Python:
/// `github_actions_workflow` uses `path_check`, others that filter use
/// `path_filter`. Semantics are identical (predicate must be true for the
/// pattern to apply); callers should treat either as "skip if false".
#[derive(Debug, Clone)]
pub struct SecurityPattern {
    pub rule_name: &'static str,
    pub path_check: Option<fn(&str) -> bool>,
    pub path_filter: Option<fn(&str) -> bool>,
    pub substrings: Option<&'static [&'static str]>,
    pub regex: Option<&'static str>,
    pub reminder: &'static str,
}

// ---------------------------------------------------------------------------
// SECURITY_PATTERNS — mirrors patterns.py:53-284 (25 entries, order preserved)
// ---------------------------------------------------------------------------

pub static SECURITY_PATTERNS: &[SecurityPattern] = &[
    SecurityPattern {
        rule_name: "github_actions_workflow",
        path_check: Some(is_github_workflow_file),
        path_filter: None,
        substrings: None,
        regex: None,
        reminder: GITHUB_WORKFLOW_REMINDER,
    },
    SecurityPattern {
        rule_name: "child_process_exec",
        path_check: None,
        path_filter: Some(is_js_file),
        substrings: Some(&["child_process.exec", "execSync("]),
        regex: Some(r#"(?<![a-zA-Z0-9_\.])exec\("#),
        reminder: CHILD_PROCESS_EXEC_REMINDER,
    },
    SecurityPattern {
        rule_name: "new_function_injection",
        path_check: None,
        path_filter: None,
        substrings: Some(&["new Function"]),
        regex: None,
        reminder: r#"⚠️ Security Warning: Using new Function() with string interpolation is a CODE INJECTION vulnerability. If any variable is concatenated or interpolated into the function body string, an attacker controlling that variable can execute arbitrary code. Use safe alternatives: for property access use obj[key] or array.reduce((o, k) => o[k], root); for computation use a safe expression parser. NEVER interpolate untrusted strings into new Function() bodies."#,
    },
    SecurityPattern {
        rule_name: "eval_injection",
        path_check: None,
        path_filter: Some(is_not_doc_file),
        substrings: None,
        regex: Some(r#"(?<![a-zA-Z0-9_\.])eval\("#),
        reminder: r#"⚠️ Security Warning: eval() executes arbitrary code and is a major security risk. Use JSON.parse() for data, ast.literal_eval() for Python literals, or a safe expression parser. If this is safe or is explicitly needed, briefly document that in a comment before continuing."#,
    },
    SecurityPattern {
        rule_name: "react_dangerously_set_html",
        path_check: None,
        path_filter: None,
        substrings: Some(&["dangerouslySetInnerHTML"]),
        regex: None,
        reminder: r#"⚠️ Security Warning: dangerouslySetInnerHTML can lead to XSS vulnerabilities if used with untrusted content. Ensure all content is properly sanitized using an HTML sanitizer library like DOMPurify, or use safe alternatives."#,
    },
    SecurityPattern {
        rule_name: "document_write_xss",
        path_check: None,
        path_filter: None,
        substrings: Some(&["document.write"]),
        regex: None,
        reminder: r#"⚠️ Security Warning: document.write() can be exploited for XSS attacks and has performance issues. Use DOM manipulation methods like createElement() and appendChild() instead."#,
    },
    SecurityPattern {
        rule_name: "innerHTML_xss",
        path_check: None,
        path_filter: None,
        substrings: Some(&[".innerHTML =", ".innerHTML="]),
        regex: None,
        reminder: r#"⚠️ Security Warning: Setting innerHTML with untrusted content can lead to XSS vulnerabilities. Use textContent for plain text or safe DOM methods for HTML content. If you need HTML support, consider using an HTML sanitizer library such as DOMPurify."#,
    },
    SecurityPattern {
        rule_name: "pickle_deserialization",
        path_check: None,
        path_filter: Some(is_py_file),
        substrings: None,
        regex: Some(r#"(?<![a-zA-Z0-9_])pickle\.(loads?|Unpickler)\b|(?<![a-zA-Z0-9_])pkl_load\("#),
        reminder: UNSAFE_DESERIALIZATION_REMINDER,
    },
    SecurityPattern {
        rule_name: "os_system_injection",
        path_check: None,
        path_filter: Some(is_py_file),
        substrings: Some(&["from os import system"]),
        regex: Some(r#"\bos\.system\s*\("#),
        reminder: r#"⚠️ Security Warning: os.system() runs a shell and is a command-injection sink. Use subprocess.run([...]) with a list of arguments instead. If this is safe or is explicitly needed, briefly document that in a comment before continuing."#,
    },
    SecurityPattern {
        rule_name: "python_subprocess_shell",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"subprocess\.(?:run|call|Popen|check_output|check_call)\(.*shell\s*=\s*True"#),
        reminder: PYTHON_SUBPROCESS_SHELL_REMINDER,
    },
    SecurityPattern {
        rule_name: "go_exec_shell_injection",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"exec\.Command\(\s*"(?:sh|bash|/bin/sh|/bin/bash)""#),
        reminder: GO_EXEC_SHELL_INJECTION_REMINDER,
    },
    SecurityPattern {
        rule_name: "unsafe_yaml_load",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"\byaml\.load\s*\((?![^)\n]{0,80}\bSafe)"#),
        reminder: UNSAFE_YAML_LOAD_REMINDER,
    },
    SecurityPattern {
        rule_name: "node_createcipher_no_iv",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"\bcrypto\.(createCipher|createDecipher)\b"#),
        reminder: r#"⚠️ Security Warning: Use crypto.createCipheriv() / createDecipheriv(). createCipher was removed in Node 22 and derives the key insecurely (no IV, MD5-based KDF)."#,
    },
    SecurityPattern {
        rule_name: "aes_ecb_mode",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"\bAES\.MODE_ECB\b|\bmodes\.ECB\s*\(|[\x22\x27]aes-\d+-ecb[\x22\x27]"#),
        reminder: r#"⚠️ Security Warning: Use AES-GCM or AES-CBC with HMAC. ECB mode leaks plaintext structure (identical blocks encrypt to identical ciphertext)."#,
    },
    SecurityPattern {
        rule_name: "tls_verification_disabled",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"\bverify\s*=\s*False\b|rejectUnauthorized\s*:\s*false|InsecureSkipVerify\s*:\s*true|NODE_TLS_REJECT_UNAUTHORIZED\s*=\s*[\x22\x27]?0|ssl\._create_unverified_context|check_hostname\s*=\s*False"#),
        reminder: r#"⚠️ Security Warning: Don't disable TLS verification. This allows MITM attacks. For self-signed dev certs, add the CA to your trust store or use a properly-issued cert."#,
    },
    SecurityPattern {
        rule_name: "marshal_loads",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"\bmarshal\.loads?\s*\("#),
        reminder: UNSAFE_DESERIALIZATION_REMINDER,
    },
    SecurityPattern {
        rule_name: "shelve_open",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"\bshelve\.open\s*\("#),
        reminder: UNSAFE_DESERIALIZATION_REMINDER,
    },
    SecurityPattern {
        rule_name: "xml_unsafe_parse",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"\b(xml\.etree\.ElementTree|ElementTree|ET)\.(parse|fromstring|XML)\s*\(|\bminidom\.(parse|parseString)\s*\(|\bxml\.sax\.(parse|make_parser)\b"#),
        reminder: r#"⚠️ Security Warning: Use defusedxml.ElementTree. Python's stdlib XML parsers are vulnerable to XXE (external entity) and billion-laughs attacks by default."#,
    },
    SecurityPattern {
        rule_name: "pickle_variants_load",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"\b(cPickle|cloudpickle|dill)\.(load|loads)\s*\("#),
        reminder: UNSAFE_DESERIALIZATION_REMINDER,
    },
    SecurityPattern {
        rule_name: "outerHTML_xss",
        path_check: None,
        path_filter: None,
        substrings: Some(&[".outerHTML =", ".outerHTML="]),
        regex: None,
        reminder: r#"⚠️ Security Warning: Use textContent or sanitize with DOMPurify. outerHTML assignment is an XSS sink equivalent to innerHTML."#,
    },
    SecurityPattern {
        rule_name: "insertAdjacentHTML_xss",
        path_check: None,
        path_filter: None,
        substrings: Some(&[".insertAdjacentHTML("]),
        regex: None,
        reminder: r#"⚠️ Security Warning: Use insertAdjacentText() or sanitize with DOMPurify. insertAdjacentHTML is an XSS sink."#,
    },
    SecurityPattern {
        rule_name: "script_src_without_sri",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"<script\s+(?![^>]{0,400}integrity\s*=)[^>]{0,200}src\s*=\s*[\x22\x27](?:https?:)?//[^\x22\x27]{1,300}[\x22\x27][^>]{0,100}>"#),
        reminder: r#"⚠️ Security Warning: Add integrity="sha384-..." crossorigin="anonymous" to external script tags. Loading scripts without Subresource Integrity exposes you to CDN compromise."#,
    },
    SecurityPattern {
        rule_name: "torch_unsafe_load",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"(?:\btorch\.load|\.torch_load)\s*\((?![^)\n]{0,200}weights_only\s*=\s*True)"#),
        reminder: UNSAFE_TORCH_LOAD_REMINDER,
    },
    SecurityPattern {
        rule_name: "yaml_unsafe_load_variants",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"(?:\byaml\.unsafe_load|\.yaml_unsafe_load)\s*\("#),
        reminder: UNSAFE_YAML_LOAD_REMINDER,
    },
    SecurityPattern {
        rule_name: "pickle_wrapper_load",
        path_check: None,
        path_filter: None,
        substrings: None,
        regex: Some(r#"\bjoblib\.load\s*\(|\b(?:pd|pandas)\.read_pickle\s*\(|\.cloudpickle_load\s*\(|\b(?:np|numpy)\.load\s*\([^)\n]{0,200}allow_pickle\s*=\s*True"#),
        reminder: UNSAFE_DESERIALIZATION_REMINDER,
    },
];

// ---------------------------------------------------------------------------
// RuleId — mirrors patterns.py:287-320 (IntEnum, stable IDs, append-only)
// ---------------------------------------------------------------------------

/// Stable numeric IDs for SECURITY_PATTERNS rules, emitted via metrics.
/// Values are frozen: do not renumber existing entries. Append new ones.
/// Mirrors `class RuleId(IntEnum)` (patterns.py:287-320).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum RuleId {
    GithubActionsWorkflow = 1,
    ChildProcessExec = 2,
    NewFunctionInjection = 3,
    EvalInjection = 4,
    ReactDangerouslySetHtml = 5,
    DocumentWriteXss = 6,
    InnerHtmlXss = 7,
    PickleDeserialization = 8,
    OsSystemInjection = 9,
    PythonSubprocessShell = 10,
    GoExecShellInjection = 11,
    UnsafeYamlLoad = 12,
    NodeCreatecipherNoIv = 13,
    AesEcbMode = 14,
    TlsVerificationDisabled = 15,
    MarshalLoads = 16,
    ShelveOpen = 17,
    XmlUnsafeParse = 18,
    PickleVariantsLoad = 19,
    OuterHtmlXss = 20,
    InsertAdjacentHtmlXss = 21,
    ScriptSrcWithoutSri = 22,
    TorchUnsafeLoad = 23,
    YamlUnsafeLoadVariants = 24,
    PickleWrapperLoad = 25,
}

// ---------------------------------------------------------------------------
// _RULE_NAME_TO_ID — mirrors patterns.py:323-349
// ---------------------------------------------------------------------------

/// Mirrors `_RULE_NAME_TO_ID = { ... }` (patterns.py:323-349).
pub const RULE_NAME_TO_ID: &[(&str, RuleId)] = &[
    ("github_actions_workflow", RuleId::GithubActionsWorkflow),
    ("child_process_exec", RuleId::ChildProcessExec),
    ("new_function_injection", RuleId::NewFunctionInjection),
    ("eval_injection", RuleId::EvalInjection),
    ("react_dangerously_set_html", RuleId::ReactDangerouslySetHtml),
    ("document_write_xss", RuleId::DocumentWriteXss),
    ("innerHTML_xss", RuleId::InnerHtmlXss),
    ("pickle_deserialization", RuleId::PickleDeserialization),
    ("os_system_injection", RuleId::OsSystemInjection),
    ("python_subprocess_shell", RuleId::PythonSubprocessShell),
    ("go_exec_shell_injection", RuleId::GoExecShellInjection),
    ("unsafe_yaml_load", RuleId::UnsafeYamlLoad),
    ("node_createcipher_no_iv", RuleId::NodeCreatecipherNoIv),
    ("aes_ecb_mode", RuleId::AesEcbMode),
    ("tls_verification_disabled", RuleId::TlsVerificationDisabled),
    ("marshal_loads", RuleId::MarshalLoads),
    ("shelve_open", RuleId::ShelveOpen),
    ("xml_unsafe_parse", RuleId::XmlUnsafeParse),
    ("pickle_variants_load", RuleId::PickleVariantsLoad),
    ("outerHTML_xss", RuleId::OuterHtmlXss),
    ("insertAdjacentHTML_xss", RuleId::InsertAdjacentHtmlXss),
    ("script_src_without_sri", RuleId::ScriptSrcWithoutSri),
    ("torch_unsafe_load", RuleId::TorchUnsafeLoad),
    ("yaml_unsafe_load_variants", RuleId::YamlUnsafeLoadVariants),
    ("pickle_wrapper_load", RuleId::PickleWrapperLoad),
];

/// Lookup RuleId by rule name — mirrors `_RULE_NAME_TO_ID[name]`.
pub fn rule_id_for_name(name: &str) -> Option<RuleId> {
    for (n, id) in RULE_NAME_TO_ID {
        if *n == name {
            return Some(*id);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Sync assertion — mirrors patterns.py:351-357
// ---------------------------------------------------------------------------

/// Assert `SECURITY_PATTERNS` and `RULE_NAME_TO_ID` are in sync.
/// Mirrors the `assert set(_RULE_NAME_TO_ID) == {p["ruleName"] for p in SECURITY_PATTERNS}` at import time.
/// Call at startup or in tests; panics with diagnostic if desync is detected.
pub fn assert_patterns_sync() {
    let pattern_names: HashSet<&str> = SECURITY_PATTERNS.iter().map(|p| p.rule_name).collect();
    let map_names: HashSet<&str> = RULE_NAME_TO_ID.iter().map(|(n, _)| *n).collect();
    let missing: HashSet<&&str> = pattern_names.difference(&map_names).collect();
    let extra: HashSet<&&str> = map_names.difference(&pattern_names).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "RuleId enum out of sync with SECURITY_PATTERNS: missing={:?}, extra={:?}",
        missing,
        extra
    );
    assert_eq!(
        SECURITY_PATTERNS.len(),
        RULE_NAME_TO_ID.len(),
        "SECURITY_PATTERNS len {} != RULE_NAME_TO_ID len {}",
        SECURITY_PATTERNS.len(),
        RULE_NAME_TO_ID.len()
    );
}

// ---------------------------------------------------------------------------
// rule_names_to_mask — mirrors patterns.py:360-368
// ---------------------------------------------------------------------------

/// Pack a set of rule names into a bitmask. Bit N set means RuleId(N) matched.
/// User-defined patterns (rule_name starting with "user:") have no static
/// RuleId and are excluded from the mask (via map miss).
/// Mirrors `def rule_names_to_mask(rule_names):` (patterns.py:360-368).
pub fn rule_names_to_mask<I, S>(rule_names: I) -> u64
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut mask: u64 = 0;
    for name in rule_names {
        let name_ref = name.as_ref();
        // user: prefix has no RuleId — excluded via None (same as Python's `if name in _RULE_NAME_TO_ID`)
        if let Some(id) = rule_id_for_name(name_ref) {
            mask |= 1u64 << (id as u64);
        }
    }
    mask
}

/// Convenience wrapper for `&[&str]` slices — mirrors common call site `rule_names_to_mask(set_of_names)`.
pub fn rule_names_to_mask_slice(rule_names: &[&str]) -> u64 {
    rule_names_to_mask(rule_names.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_len_is_25() {
        assert_eq!(SECURITY_PATTERNS.len(), 25);
        assert_eq!(RULE_NAME_TO_ID.len(), 25);
    }

    #[test]
    fn sync_assert_passes() {
        assert_patterns_sync();
    }

    #[test]
    fn rule_id_lookup() {
        assert_eq!(rule_id_for_name("github_actions_workflow"), Some(RuleId::GithubActionsWorkflow));
        assert_eq!(rule_id_for_name("pickle_wrapper_load"), Some(RuleId::PickleWrapperLoad));
        assert_eq!(rule_id_for_name("nonexistent"), None);
        assert_eq!(rule_id_for_name("user:custom_rule"), None);
    }

    #[test]
    fn mask_packs_bits() {
        // Bit N set means RuleId(N) matched — mirrors Python `1 << RuleId`
        let mask = rule_names_to_mask(["github_actions_workflow", "eval_injection"]);
        assert_eq!(mask, (1u64 << RuleId::GithubActionsWorkflow as u64) | (1u64 << RuleId::EvalInjection as u64));
        assert_eq!(mask, (1u64 << 1) | (1u64 << 4));
        // user: prefix excluded
        let mask2 = rule_names_to_mask(["user:my_custom", "eval_injection"]);
        assert_eq!(mask2, 1u64 << RuleId::EvalInjection as u64);
        // empty
        let empty: &[&str] = &[];
        assert_eq!(rule_names_to_mask(empty.iter().copied()), 0);
        // duplicates don't double-set
        let mask3 = rule_names_to_mask(["eval_injection", "eval_injection"]);
        assert_eq!(mask3, 1u64 << RuleId::EvalInjection as u64);
    }

    #[test]
    fn mask_slice_wrapper() {
        assert_eq!(rule_names_to_mask_slice(&["github_actions_workflow"]), 1u64 << 1);
        assert_eq!(rule_names_to_mask_slice(&[]), 0);
    }

    #[test]
    fn js_ext_predicate() {
        assert!(is_js_file("app.js"));
        assert!(is_js_file("component.vue"));
        assert!(is_js_file("file.mjs"));
        assert!(!is_js_file("script.py"));
        assert!(!is_js_file("README.md"));
    }

    #[test]
    fn py_ext_predicate() {
        assert!(is_py_file("script.py"));
        assert!(is_py_file("notebook.ipynb"));
        assert!(!is_py_file("app.js"));
    }

    #[test]
    fn not_doc_predicate() {
        assert!(!is_not_doc_file("README.md"));
        assert!(!is_not_doc_file("data.json"));
        assert!(is_not_doc_file("app.js"));
        assert!(is_not_doc_file("script.py"));
    }

    #[test]
    fn github_workflow_predicate() {
        assert!(is_github_workflow_file(".github/workflows/ci.yml"));
        assert!(is_github_workflow_file(".github/workflows/deploy.yaml"));
        assert!(is_github_workflow_file("repo/.github/workflows/test.yml"));
        assert!(!is_github_workflow_file(".github/workflows/README.md"));
        assert!(!is_github_workflow_file("src/main.yml"));
    }

    #[test]
    fn pattern_order_matches_rule_id() {
        // SECURITY_PATTERNS order must match RuleId discriminants 1..25
        for (idx, pat) in SECURITY_PATTERNS.iter().enumerate() {
            let expected_id = (idx + 1) as u32;
            let id = rule_id_for_name(pat.rule_name).unwrap() as u32;
            assert_eq!(id, expected_id, "pattern {} order mismatch: {} has id {}", idx, pat.rule_name, id);
        }
    }

    #[test]
    fn all_reminders_non_empty() {
        for pat in SECURITY_PATTERNS {
            assert!(!pat.reminder.is_empty(), "reminder empty for {}", pat.rule_name);
            assert!(pat.reminder.contains("Security Warning") || pat.reminder.contains("⚠️"), "reminder missing prefix for {}", pat.rule_name);
        }
    }

    #[test]
    fn substrings_and_regex_coverage() {
        // child_process has both
        let cp = &SECURITY_PATTERNS[1];
        assert_eq!(cp.rule_name, "child_process_exec");
        assert!(cp.substrings.is_some());
        assert!(cp.regex.is_some());
        assert!(cp.path_filter.is_some());
        // github has only path_check
        let gh = &SECURITY_PATTERNS[0];
        assert!(gh.path_check.is_some());
        assert!(gh.path_filter.is_none());
        assert!(gh.substrings.is_none());
        assert!(gh.regex.is_none());
    }
}
