//! Bounded, curated Markdown memory. One GRAY_HOME is one trusted owner.
//! Commands use the existing bash surface; no extraction service or new tool.
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use clap::{Args, Subcommand, ValueEnum};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Scope {
    User,
    #[default]
    Project,
}

#[derive(Args, Debug, Clone)]
pub struct MemoryArgs {
    /// User preferences or current project's confirmed decisions
    #[arg(long, value_enum, default_value = "project", global = true)]
    pub scope: Scope,
    #[command(subcommand)]
    pub command: MemoryCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum MemoryCommand {
    /// Show the current curated entries
    List,
    /// Show one entry
    Show { key: String },
    /// Add or replace a named entry (one line; never credentials)
    Set { key: String, text: String },
    /// Rewrite an existing entry's text (fails when the key is unknown)
    Edit { key: String, text: String },
    /// Forget a named entry for future sessions
    Remove { key: String },
    /// Forget every entry in the scope
    Clear,
}

pub fn disabled() -> bool {
    std::env::var("GRAY_NO_MEMORY").is_ok_and(|v| {
        !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        )
    })
}

pub fn run_cli(args: &MemoryArgs) -> anyhow::Result<()> {
    let store = MemoryStore::new(&crate::setup::gray_home()?, &std::env::current_dir()?)?;
    match &args.command {
        MemoryCommand::List => {
            let text = store.list(args.scope)?;
            if text.is_empty() {
                println!("No memories.");
            } else {
                print!("{text}");
            }
        }
        MemoryCommand::Show { key } => match store.get(args.scope, key)? {
            Some(text) => println!("- {key}: {text}"),
            None => println!("No entry named '{key}'."),
        },
        MemoryCommand::Set { key, text } => {
            ensure!(!disabled(), "memory saving disabled by GRAY_NO_MEMORY");
            let changed = store.set(args.scope, key, text)?;
            println!(
                "{}",
                if changed {
                    "Memory updated."
                } else {
                    "Memory unchanged."
                }
            );
        }
        MemoryCommand::Edit { key, text } => {
            ensure!(!disabled(), "memory saving disabled by GRAY_NO_MEMORY");
            store.edit(args.scope, key, text)?;
            println!("Memory edited.");
        }
        MemoryCommand::Remove { key } => {
            store.remove(args.scope, key)?;
            println!(
                "Memory removed. Existing sessions and transcripts retain their earlier context."
            );
        }
        MemoryCommand::Clear => {
            ensure!(!disabled(), "memory saving disabled by GRAY_NO_MEMORY");
            let dropped = store.clear(args.scope)?;
            println!(
                "{}",
                match dropped {
                    0 => "No memories to clear.".to_string(),
                    1 => "Memory cleared (1 entry).".to_string(),
                    n => format!("Memory cleared ({n} entries)."),
                }
            );
        }
    }
    Ok(())
}

pub struct MemoryStore {
    root: PathBuf,
    project: String,
}

impl MemoryStore {
    pub fn new(home: &Path, cwd: &Path) -> anyhow::Result<Self> {
        let cwd = cwd
            .canonicalize()
            .context("cannot resolve memory project directory")?;
        let project_root = crate::skills::find_git_root(&cwd).unwrap_or(cwd);
        let project = format!(
            "{:x}",
            Sha256::digest(project_root.as_os_str().as_encoded_bytes())
        );
        Ok(Self {
            root: home.join("memory"),
            project,
        })
    }

    fn path(&self, scope: Scope) -> PathBuf {
        match scope {
            Scope::User => self.root.join("user.md"),
            Scope::Project => self.root.join(format!("project-{}.md", self.project)),
        }
    }

    /// Total curated entries across both scopes; an unreadable or missing
    /// store reads as empty (a missing store *is* an empty store).
    pub fn entry_count(&self) -> usize {
        [Scope::User, Scope::Project]
            .into_iter()
            .map(|s| {
                parse(&self.list(s).unwrap_or_default())
                    .map(|e| e.len())
                    .unwrap_or(0)
            })
            .sum()
    }

    /// Freeze curated data, not instructions, once per durable session. A new
    /// process rebuilding the same session gets identical bytes. Anonymous
    /// headless runs take a fresh snapshot and leave no snapshot file.
    pub fn snapshot(&self, session: Option<&str>) -> anyhow::Result<String> {
        let capture = || -> anyhow::Result<String> {
            Ok(serde_json::to_string(&serde_json::json!({
                "project": self.project,
                "user": self.list(Scope::User)?,
                "decisions": self.list(Scope::Project)?,
            }))?)
        };
        let Some(id) = session else {
            return capture();
        };
        let id = uuid::Uuid::parse_str(id)
            .context("invalid memory session id")?
            .to_string();
        let dir = self.root.join("snapshots");
        private_dir(&dir)?;
        let path = dir.join(format!("{id}.json"));
        let _lock = lock(&dir.join(format!("{id}.lock")))?;
        if let Some(text) = read_text(&path)? {
            let value: serde_json::Value =
                serde_json::from_str(&text).context("invalid memory snapshot")?;
            ensure!(
                value["project"].as_str() == Some(&self.project),
                "memory snapshot belongs to another project"
            );
            for field in ["user", "decisions"] {
                let content = value[field]
                    .as_str()
                    .context("invalid memory snapshot fields")?;
                parse(content)?;
            }
            // Return canonical fields only, never arbitrary extra snapshot data.
            return Ok(serde_json::to_string(&serde_json::json!({
                "project": self.project,
                "user": value["user"],
                "decisions": value["decisions"],
            }))?);
        }
        let text = capture()?;
        atomic_write(&path, &text)?;
        Ok(text)
    }

    pub fn list(&self, scope: Scope) -> anyhow::Result<String> {
        let text = read_text(&self.path(scope))?.unwrap_or_default();
        Ok(render(&parse(&text)?))
    }

    pub fn set(&self, scope: Scope, key: &str, text: &str) -> anyhow::Result<bool> {
        validate_key(key)?;
        validate_text(text)?;
        self.change(scope, |entries| {
            if entries.get(key).is_some_and(|old| old == text.trim())
                || (!entries.contains_key(key) && entries.values().any(|v| v == text.trim()))
            {
                return Ok(false);
            }
            entries.insert(key.to_owned(), text.trim().to_owned());
            Ok(true)
        })
    }

    /// One entry's text, if the scope holds it.
    pub fn get(&self, scope: Scope, key: &str) -> anyhow::Result<Option<String>> {
        validate_key(key)?;
        Ok(parse(&self.list(scope)?)?.remove(key))
    }

    /// Rewrite the text of an existing entry; never creates a new one.
    pub fn edit(&self, scope: Scope, key: &str, text: &str) -> anyhow::Result<()> {
        validate_key(key)?;
        validate_text(text)?;
        let trimmed = text.trim().to_owned();
        self.change(scope, |entries| {
            ensure!(entries.contains_key(key), "no memory entry named '{key}'");
            if entries.get(key).is_some_and(|old| *old == trimmed) {
                return Ok(false);
            }
            entries.insert(key.to_owned(), trimmed);
            Ok(true)
        })?;
        Ok(())
    }

    /// Remove every entry in the scope; returns how many were dropped.
    pub fn clear(&self, scope: Scope) -> anyhow::Result<usize> {
        let mut dropped = 0;
        self.change(scope, |entries| {
            dropped = entries.len();
            entries.clear();
            Ok(dropped > 0)
        })?;
        Ok(dropped)
    }

    pub fn remove(&self, scope: Scope, key: &str) -> anyhow::Result<()> {
        validate_key(key)?;
        self.change(scope, |entries| {
            ensure!(entries.remove(key).is_some(), "memory entry not found");
            Ok(true)
        })?;
        Ok(())
    }

    fn change(
        &self,
        scope: Scope,
        update: impl FnOnce(&mut BTreeMap<String, String>) -> anyhow::Result<bool>,
    ) -> anyhow::Result<bool> {
        private_dir(&self.root)?;
        let path = self.path(scope);
        let _lock = lock(&path.with_extension("lock"))?;
        let mut entries = parse(&self.list(scope)?)?;
        let changed = update(&mut entries)?;
        if changed {
            atomic_write(&path, &render(&entries))?;
        }
        Ok(changed)
    }
}

fn validate_key(key: &str) -> anyhow::Result<()> {
    ensure!(
        !key.is_empty()
            && key.len() <= 64
            && key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
        "memory key must be 1-64 ASCII letters, digits, underscores or hyphens"
    );
    Ok(())
}

fn validate_text(text: &str) -> anyhow::Result<()> {
    ensure!(!text.trim().is_empty(), "memory text must not be empty");
    ensure!(!text.chars().any(|c| c.is_control() || matches!(c, '\u{00ad}' | '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')), "memory text must be one line without control or invisible formatting characters");
    let redacted = gray_core::redaction::redact_for_disclosure(text);
    ensure!(
        !redacted
            .kinds()
            .iter()
            .any(|k| k == gray_core::redaction::REDACTION_SECRET),
        "memory text contains a possible credential; not saved"
    );
    Ok(())
}

fn parse(text: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let mut entries = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let (key, value) = line
            .strip_prefix("- ")
            .and_then(|l| l.split_once(": "))
            .context("invalid memory Markdown; expected '- key: text'")?;
        validate_key(key)?;
        validate_text(value)?;
        ensure!(
            entries
                .insert(key.to_owned(), value.trim().to_owned())
                .is_none(),
            "duplicate key in memory file"
        );
    }
    Ok(entries)
}

fn render(entries: &BTreeMap<String, String>) -> String {
    entries
        .iter()
        .map(|(k, v)| format!("- {k}: {v}\n"))
        .collect()
}

/// Refuse symlinks in managed paths, including ancestors; no path supplied by
/// memory text is ever opened. This is not a sandbox against the same OS user.
fn reject_symlinks(path: &Path) -> anyhow::Result<()> {
    for p in path.ancestors() {
        match std::fs::symlink_metadata(p) {
            Ok(m) => ensure!(
                !m.file_type().is_symlink(),
                "memory path contains a symbolic link"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("cannot inspect memory path"),
        }
    }
    Ok(())
}

fn private_dir(path: &Path) -> anyhow::Result<()> {
    reject_symlinks(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(path)?;
    Ok(())
}

/// Read a memory file whole. There is no size cap: the file belongs to the
/// same OS user and every entry was validated on the way in.
fn read_text(path: &Path) -> anyhow::Result<Option<String>> {
    reject_symlinks(path)?;
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("cannot read memory file"),
    };
    ensure!(
        file.metadata()?.is_file(),
        "memory path is not a regular file"
    );
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(
        String::from_utf8(bytes).context("memory file is not UTF-8")?,
    ))
}

fn lock(path: &Path) -> anyhow::Result<File> {
    reject_symlinks(path)?;
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                ensure!(
                    std::time::Instant::now() < deadline,
                    "memory lock timeout; retry later"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e).context("cannot lock memory"),
        }
    }
}

fn atomic_write(path: &Path, text: &str) -> anyhow::Result<()> {
    reject_symlinks(path)?;
    let parent = path.parent().context("memory path has no parent")?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    // NamedTempFile is owner-only on Unix; persist replaces atomically on
    // supported platforms and removes the tempfile on error via RAII.
    tmp.write_all(text.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .map_err(|e| e.error)
        .context("cannot persist memory")?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
