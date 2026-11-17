mod edit;
mod patch;
mod read;
mod search;

use std::io;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use gray_core::tool_out::{get_str, resolve_path};
use serde_json::{Value, json};

pub const ENV: &str = "GRAY_FILE_TOOLS";

type R = Result<String, ToolOutput>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialect {
    Plain,
    Claude,
    Codex,
}

impl Dialect {
    pub fn current() -> Option<Self> {
        Self::parse(&std::env::var(ENV).ok()?)
    }

    pub fn parse(v: &str) -> Option<Self> {
        match v.trim().to_ascii_lowercase().as_str() {
            "" | "0" | "off" | "false" | "none" => None,
            "claude" => Some(Self::Claude),
            "codex" | "patch" | "apply_patch" => Some(Self::Codex),
            _ => Some(Self::Plain),
        }
    }

    pub fn for_model(model: &str) -> Self {
        let id = model
            .rsplit('/')
            .next()
            .unwrap_or(model)
            .to_ascii_lowercase();
        if ["gpt", "codex", "o1", "o3", "o4"]
            .iter()
            .any(|f| id.starts_with(f))
        {
            Self::Codex
        } else {
            Self::Claude
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn guidance(self) -> &'static str {
        match self {
            Self::Plain => {
                "Use read, edit, write, grep and glob for files instead of cat, sed, heredocs or find; use bash to run programs."
            }
            Self::Claude => {
                "Use Read, Edit, Write, Grep and Glob for files instead of cat, sed, heredocs or find; use bash to run programs."
            }
            Self::Codex => {
                "Edit files with apply_patch, never with sed, heredocs or scripts that rewrite files; read and search with bash (rg, sed -n, cat)."
            }
        }
    }
}

pub fn resolve_auto(model: &str) {
    if std::env::var(ENV).is_ok_and(|v| v.trim().eq_ignore_ascii_case("auto")) {
        unsafe { std::env::set_var(ENV, Dialect::for_model(model).name()) };
    }
}

pub fn guidance() -> Option<&'static str> {
    Dialect::current().map(Dialect::guidance)
}

#[derive(Clone, Copy)]
enum Kind {
    Read,
    Edit,
    Write,
    Grep,
    Glob,
    Patch,
}

struct FileTool(Kind, Dialect);

pub fn tools() -> Vec<Arc<dyn Tool>> {
    let Some(d) = Dialect::current() else {
        return Vec::new();
    };
    let kinds: &[Kind] = match d {
        Dialect::Codex => &[Kind::Patch],
        _ => &[Kind::Read, Kind::Edit, Kind::Write, Kind::Grep, Kind::Glob],
    };
    kinds
        .iter()
        .map(|&k| Arc::new(FileTool(k, d)) as Arc<dyn Tool>)
        .collect()
}

fn plain_def(kind: Kind) -> (&'static str, &'static str, Value) {
    match kind {
        Kind::Read => (
            "read",
            "Read a text file. Returns lines as `N: text` from `offset` (1-based, default 1), at most `limit` lines (default and max 2000) or 50 KB; lines longer than 2000 characters are cut. Read every file you need in one turn. Use this instead of cat, head or sed to view files.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer"},
                    "limit": {"type": "integer"}
                },
                "required": ["path"]
            }),
        ),
        Kind::Edit => (
            "edit",
            "Replace exact text in a file. Every oldText is matched against the file as it was before this call, must match exactly one place, and must not overlap another edit. Put all changes to one file in one call. Keep each oldText short but unique, copied from read output without the `N: ` prefix. If any edit fails, nothing is written.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "oldText": {"type": "string"},
                                "newText": {"type": "string"}
                            },
                            "required": ["oldText", "newText"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }),
        ),
        Kind::Write => (
            "write",
            "Create a file or replace its whole content; parent directories are created. Use edit to change part of an existing file.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }),
        ),
        Kind::Grep => (
            "grep",
            "Search file contents with a regular expression (Rust regex syntax). Skips .gitignore'd and hidden files. Returns `path:line: text`, at most 100 matches. `path` defaults to the working directory; `glob` filters file names, e.g. `*.rs`.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                    "glob": {"type": "string"},
                    "ignore_case": {"type": "boolean"}
                },
                "required": ["pattern"]
            }),
        ),
        Kind::Glob => (
            "glob",
            "Find files by glob pattern, e.g. `src/**/*.ts`. Skips .gitignore'd and hidden files. Returns up to 100 paths, most recently modified first.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"}
                },
                "required": ["pattern"]
            }),
        ),
        Kind::Patch => (
            "apply_patch",
            patch::DESCRIPTION,
            json!({
                "type": "object",
                "properties": {"input": {"type": "string"}},
                "required": ["input"]
            }),
        ),
    }
}

fn claude_def(kind: Kind) -> (&'static str, &'static str, Value) {
    match kind {
        Kind::Read => (
            "Read",
            "Reads a file from the local filesystem. file_path may be absolute or relative to the working directory. By default reads up to 2000 lines from the start; pass offset (1-based line) and limit for long files. Output is cat -n style: line number, a tab, then the line. Lines over 2000 characters are cut. Reading a directory lists it. Read every file you need in one turn.",
            json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "The path to the file to read"},
                    "offset": {"type": "integer", "description": "The line number to start reading from"},
                    "limit": {"type": "integer", "description": "The number of lines to read"}
                },
                "required": ["file_path"]
            }),
        ),
        Kind::Edit => (
            "Edit",
            "Performs exact string replacement in a file. old_string must match the file exactly once (copy it from Read output without the line-number prefix); include more surrounding lines if it is not unique, or set replace_all to change every occurrence. Fails without writing if old_string is missing or ambiguous.",
            json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "The path to the file to modify"},
                    "old_string": {"type": "string", "description": "The text to replace"},
                    "new_string": {"type": "string", "description": "The text to replace it with (must differ from old_string)"},
                    "replace_all": {"type": "boolean", "default": false, "description": "Replace all occurrences of old_string"}
                },
                "required": ["file_path", "old_string", "new_string"]
            }),
        ),
        Kind::Write => (
            "Write",
            "Writes a file, replacing it if it exists; parent directories are created. Prefer Edit for changes to an existing file.",
            json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "The path to the file to write"},
                    "content": {"type": "string", "description": "The content to write to the file"}
                },
                "required": ["file_path", "content"]
            }),
        ),
        Kind::Grep => (
            "Grep",
            "Searches file contents with a regular expression (ripgrep-style, Rust regex syntax), skipping .gitignore'd and hidden files. output_mode: \"files_with_matches\" (default) lists matching files, \"content\" shows `path:line: text`, \"count\" shows matches per file. At most 100 results.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "The regular expression to search for"},
                    "path": {"type": "string", "description": "File or directory to search in; defaults to the working directory"},
                    "glob": {"type": "string", "description": "Glob to filter files, e.g. \"*.js\" or \"src/**/*.ts\""},
                    "output_mode": {"type": "string", "enum": ["content", "files_with_matches", "count"]},
                    "-i": {"type": "boolean", "description": "Case insensitive search"}
                },
                "required": ["pattern"]
            }),
        ),
        Kind::Glob => (
            "Glob",
            "Fast file pattern matching, e.g. \"**/*.js\" or \"src/**/*.ts\". Skips .gitignore'd and hidden files. Returns up to 100 paths, most recently modified first.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "The glob pattern to match files against"},
                    "path": {"type": "string", "description": "The directory to search in; defaults to the working directory"}
                },
                "required": ["pattern"]
            }),
        ),
        Kind::Patch => plain_def(kind),
    }
}

fn from_claude(kind: Kind, mut args: Value) -> Value {
    let Some(obj) = args.as_object_mut() else {
        return args;
    };
    if let Some(p) = obj.remove("file_path") {
        obj.entry("path").or_insert(p);
    }
    match kind {
        Kind::Edit => {
            let old = obj.remove("old_string").unwrap_or(Value::Null);
            let new = obj.remove("new_string").unwrap_or(Value::Null);
            if obj.get("replace_all").and_then(Value::as_bool) == Some(true) {
                obj.insert("oldText".into(), old);
                obj.insert("newText".into(), new);
            } else {
                obj.insert("edits".into(), json!([{"oldText": old, "newText": new}]));
            }
        }
        Kind::Grep => {
            if let Some(i) = obj.remove("-i") {
                obj.insert("ignore_case".into(), i);
            }
            obj.entry("output_mode")
                .or_insert("files_with_matches".into());
        }
        _ => {}
    }
    args
}

fn claude_error(mut out: ToolOutput) -> ToolOutput {
    if out.is_error {
        out.content = out
            .content
            .replace(" for edit #1", "")
            .replace("oldText", "old_string")
            .replace("newText", "new_string")
            .replace("'edits': expected a non-empty array", "'old_string'");
    }
    out
}

#[async_trait]
impl Tool for FileTool {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn def(&self) -> ToolDef {
        let (name, description, parameters) = match self.1 {
            Dialect::Claude => claude_def(self.0),
            _ => plain_def(self.0),
        };
        let mut def = ToolDef::new(name, description, parameters);
        def.read_only = matches!(self.0, Kind::Read | Kind::Grep | Kind::Glob);
        def
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let (kind, dialect, ctx) = (self.0, self.1, ctx.clone());
        let claude = dialect == Dialect::Claude;
        let args = if claude {
            from_claude(kind, args)
        } else {
            args
        };
        let done = tokio::task::spawn_blocking(move || match kind {
            Kind::Read => read::run(&ctx.cwd, &args, claude),
            Kind::Edit if args.get("replace_all").and_then(Value::as_bool) == Some(true) => {
                edit::replace_all(&ctx.cwd, &args)
            }
            Kind::Edit => edit::run(&ctx.cwd, &args),
            Kind::Write => write(&ctx.cwd, &args),
            Kind::Grep => search::grep(&ctx, &args),
            Kind::Glob => search::glob(&ctx, &args),
            Kind::Patch => patch::run(&ctx.cwd, &args),
        })
        .await;
        let out = match done {
            Ok(Ok(text)) => ToolOutput::ok(text),
            Ok(Err(out)) => out,
            Err(e) => ToolOutput::error(format!("file tool failed: {e}")),
        };
        if claude { claude_error(out) } else { out }
    }
}

fn err<T>(msg: impl Into<String>) -> Result<T, ToolOutput> {
    Err(ToolOutput::error(msg))
}

fn write(cwd: &Path, args: &Value) -> R {
    let p = get_str(args, "path")?;
    let content = get_str(args, "content")?;
    let path = resolve_path(cwd, &p);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ToolOutput::error(format!("{p}: {e}")))?;
    }
    write_atomic(&path, content.as_bytes()).map_err(|e| ToolOutput::error(format!("{p}: {e}")))?;
    Ok(format!("Wrote {p} ({} lines).", content.lines().count()))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::other("not a file path"))?;
    let tmp = target.with_file_name(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let done = std::fs::write(&tmp, bytes).and_then(|()| {
        if let Ok(meta) = std::fs::metadata(&target) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, &target)
    });
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s.to_string(),
    }
}

fn not_found(shown: &str, path: &Path) -> ToolOutput {
    let want = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut near: Vec<String> = path
        .parent()
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            let n = n.to_lowercase();
            !want.is_empty() && (n.contains(&want) || want.contains(&n))
        })
        .collect();
    near.sort();
    let mut msg = format!("File not found: {shown}");
    if !near.is_empty() {
        let names: Vec<String> = near
            .iter()
            .take(3)
            .map(|n| Path::new(shown).with_file_name(n).display().to_string())
            .collect();
        msg.push_str(&format!("\nSimilar: {}", names.join(", ")));
    }
    ToolOutput::error(msg)
}
