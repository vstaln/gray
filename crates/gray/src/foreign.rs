//! Foreign plugin packages: make any pi-installed package work on gray,
//! with zero per-plugin code in the host.
//!
//! `gray plugin install <git-url>` already lands a package's markdown under
//! `<home>/plugins/pi/<key>/` (`skills/*/SKILL.md` for the skill loader,
//! root `*.md` as docs, `commands/*.md` for prompts). This module is the
//! generic reader for the rest of the contract every agent host shares
//! (opencode's plugin does the same three things: register commands,
//! inject the ruleset into every turn, persist a mode switch):
//!
//! - `<pkg>/AGENTS.md` → appended to every turn's system prompt
//!   (`prompt/context`). The cross-host always-on convention opencode,
//!   Cursor, and friends auto-load; ponytail ships it at its root.
//! - `<pkg>/commands/*.md` → slash commands whose bodies run as prompts
//!   (`command/run` → `Prompt`), with `$ARGUMENTS` substitution.
//! - `<pkg>/gray.json` (optional) → a tiny generic state machine: named
//!   commands write argv to a state file whose content is appended to the
//!   inject block. This is what makes `/ponytail ultra`-style switches
//!   real without any ponytail-specific code here:
//!   `{"state_file": ".mode", "state_prefix": "Level: ",
//!     "state_commands": ["ponytail"]}`.
//!
//! Package code is never executed: only `.md`/`.json` are read. Hooks,
//! MCP servers, subagent injection, and lifecycle scripts have no gray
//! equivalent and stay out of scope.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gray_core::agent::{CommandOutcome, PluginCommand, PluginHooks};

/// Always-on rules file inside a package (the cross-host convention).
const INJECT_FILE: &str = "AGENTS.md";
/// Optional generic state manifest inside a package.
const MANIFEST_FILE: &str = "gray.json";
/// Command-prompt dirs, Claude layout first (it wins a stem both declare).
const COMMAND_DIRS: [&str; 2] = ["commands", ".opencode/command"];

/// Optional `<pkg>/gray.json`: which commands persist a mode switch and
/// where. Unknown fields are ignored; a corrupt file reads as absent and
/// the package stays static.
#[derive(Default, serde::Deserialize)]
struct GrayManifest {
    #[serde(default)]
    state_file: Option<String>,
    #[serde(default)]
    state_prefix: Option<String>,
    #[serde(default)]
    state_commands: Vec<String>,
}

struct StateCfg {
    /// Absolute path (package dir + bare filename).
    file: PathBuf,
    prefix: String,
}

struct ForeignCommand {
    /// Slash name with leading slash (`/review`).
    name: String,
    file: PathBuf,
    description: String,
    set_state: bool,
}

/// One installed foreign package. Stateless across turns except through
/// files: the inject source and the state file are re-read every call, so
/// there is deliberately no cache to go stale (project-context precedent).
pub struct ForeignPlugin {
    key: String,
    dir: PathBuf,
    commands: Vec<ForeignCommand>,
    state: Option<StateCfg>,
}

fn read_manifest(dir: &Path) -> Option<(StateCfg, Vec<String>)> {
    let raw = std::fs::read_to_string(dir.join(MANIFEST_FILE)).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let m: GrayManifest = serde_json::from_str(&raw).ok()?;
    let rel = m
        .state_file
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    // Bare filename only: the state file must stay inside the package dir.
    if rel.contains('/') || rel.contains('\\') || rel == "." || rel == ".." {
        return None;
    }
    Some((
        StateCfg {
            file: dir.join(rel),
            prefix: m.state_prefix.unwrap_or_default(),
        },
        m.state_commands,
    ))
}

/// `description:` from a command file's frontmatter (skill frontmatter
/// parser, so `description: >` folding works); `""` when absent.
fn command_description(raw: &str) -> String {
    crate::skills::parse_frontmatter(raw)
        .map(|(fm, _)| fm.description.unwrap_or_default())
        .unwrap_or_default()
}

fn scan_commands(dir: &Path, state_commands: &[String]) -> Vec<ForeignCommand> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for sub in COMMAND_DIRS {
        let Ok(rd) = std::fs::read_dir(dir.join(sub)) else {
            continue;
        };
        let mut files: Vec<String> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| Path::new(n).extension().is_some_and(|e| e == "md"))
            .collect();
        files.sort();
        for file in files {
            let stem = file.strip_suffix(".md").unwrap_or(&file);
            if stem.is_empty() || !seen.insert(stem.to_string()) {
                continue;
            }
            let path = dir.join(sub).join(&file);
            let description = std::fs::read_to_string(&path)
                .map(|raw| command_description(&raw))
                .unwrap_or_default();
            out.push(ForeignCommand {
                name: format!("/{stem}"),
                file: path,
                description,
                set_state: state_commands.iter().any(|c| c == stem),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

impl ForeignPlugin {
    /// `None` when the package offers nothing foreign: no inject file and
    /// no command prompts.
    pub fn load(key: &str, dir: &Path) -> Option<Self> {
        let (state, state_commands) = read_manifest(dir).unzip();
        let commands = scan_commands(dir, state_commands.as_deref().unwrap_or(&[]));
        if !dir.join(INJECT_FILE).is_file() && commands.is_empty() {
            return None;
        }
        Some(Self {
            key: key.to_string(),
            dir: dir.to_path_buf(),
            commands,
            state,
        })
    }

    fn state_text(&self) -> Option<String> {
        let state = self.state.as_ref()?;
        let content = std::fs::read_to_string(&state.file).ok()?;
        let content = content.trim();
        if content.is_empty() {
            return None;
        }
        Some(format!("{}{}", state.prefix, content))
    }
}

#[async_trait::async_trait]
impl PluginHooks for ForeignPlugin {
    async fn prompt_context(&self) -> Option<String> {
        let rules = std::fs::read_to_string(self.dir.join(INJECT_FILE))
            .map(|s| crate::skills_tool::strip_rationale_comments(s.trim()))
            .unwrap_or_default();
        let mut rules = rules;
        if rules.chars().count() > crate::skills_tool::PROJECT_RULES_MAX_CHARS {
            let cut = rules
                .char_indices()
                .nth(crate::skills_tool::PROJECT_RULES_MAX_CHARS)
                .map(|(i, _)| i)
                .unwrap_or(rules.len());
            rules.truncate(cut);
            rules.push_str(&format!(
                "\n…[truncated — read {} for the full file]",
                self.dir.join(INJECT_FILE).display()
            ));
        }
        let state = self.state_text();
        if rules.trim().is_empty() && state.is_none() {
            return None;
        }
        let mut block = format!(
            "<foreign_plugin name=\"{key}\" source=\"{path}\">\nInstalled plugin rules, served automatically each turn. Follow them; project rules outrank them.\n\n{rules}",
            key = self.key,
            path = self.dir.join(INJECT_FILE).display(),
        );
        if let Some(state) = state {
            block.push_str(&format!("\n\n{state}"));
        }
        block.push_str("\n</foreign_plugin>");
        Some(block)
    }

    fn commands(&self) -> Vec<PluginCommand> {
        self.commands
            .iter()
            .map(|c| PluginCommand {
                name: c.name.clone(),
                description: c.description.clone(),
            })
            .collect()
    }

    async fn run_command(&self, name: &str, argv: Vec<String>) -> Option<CommandOutcome> {
        let stem = name.strip_prefix('/').unwrap_or(name);
        let cmd = self
            .commands
            .iter()
            .find(|c| c.name.strip_prefix('/').unwrap_or(&c.name) == stem)?;
        let args = argv.join(" ");
        if cmd.set_state
            && let Some(state) = &self.state
        {
            // Generic mode switch: persist argv, confirm briefly. An empty
            // argv clears the state and the suffix drops from the inject.
            std::fs::write(&state.file, &args).ok()?;
            let reply = if args.is_empty() {
                stem.to_string()
            } else {
                format!("{stem} {args}")
            };
            return Some(CommandOutcome::Say(reply));
        }
        let raw = std::fs::read_to_string(&cmd.file).ok()?;
        let body = crate::skills_tool::strip_frontmatter(&raw);
        let parent = cmd
            .file
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = crate::skills_tool::apply_substitutions(body, Some(&args), &parent);
        let text = text.trim().to_string();
        if text.is_empty() {
            return None;
        }
        Some(CommandOutcome::Prompt(text))
    }
}

/// One adapter per installed pi package that offers an inject file or
/// command prompts. Session-start scan; a missing/unreadable home reads
/// as none.
pub fn foreign_hooks() -> Vec<Arc<dyn PluginHooks>> {
    let Ok(home) = crate::setup::gray_home() else {
        return Vec::new();
    };
    foreign_hooks_in(&home.join("plugins").join("pi"))
}

fn foreign_hooks_in(pi: &Path) -> Vec<Arc<dyn PluginHooks>> {
    let Ok(rd) = std::fs::read_dir(pi) else {
        return Vec::new();
    };
    let mut dirs: Vec<(String, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?.to_string();
            Some((name, p))
        })
        .collect();
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    dirs.into_iter()
        .filter_map(|(key, dir)| ForeignPlugin::load(&key, &dir))
        .map(|p| Arc::new(p) as Arc<dyn PluginHooks>)
        .collect()
}

#[path = "foreign_tests.rs"]
#[cfg(test)]
mod tests;
