//! Consent for project-level prompts, skills and rules.
//!
//! A cloned repo can ship `.claude/commands/*.md`, `SKILL.md` files and an
//! `AGENTS.md`. The model reads them as instructions and its tools run
//! unsandboxed (see SECURITY.md), so project content loads only for projects
//! the user has trusted with `gray trust`. The list lives under the gray
//! home, never inside the project, so a repo cannot trust itself.

use std::io;
use std::path::{Path, PathBuf};

const FILE: &str = "trusted_projects.json";

/// The project a directory belongs to: its git root, or the directory itself
/// outside a repository. Canonical, so symlinks and `..` cannot alias it.
#[must_use]
pub fn project_root(dir: &Path) -> PathBuf {
    let base = crate::skills::find_git_root(dir).unwrap_or_else(|| dir.to_path_buf());
    std::fs::canonicalize(&base).unwrap_or(base)
}

/// Project roots trusted under `home`. A missing or unreadable list trusts nothing.
#[must_use]
pub fn trusted(home: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(home.join(FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<PathBuf>>(&raw).ok())
        .unwrap_or_default()
}

/// Whether project-level content under `dir` may load.
#[must_use]
pub fn is_trusted(home: &Path, dir: &Path) -> bool {
    #[cfg(test)]
    if ASSUME_TRUSTED.with(std::cell::Cell::get) {
        return true;
    }
    trusted(home).contains(&project_root(dir))
}

#[cfg(test)]
thread_local! {
    static ASSUME_TRUSTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test seam: treat every project as trusted on the calling test's thread only.
/// The gate itself is tested directly in `project_trust_tests`.
#[cfg(test)]
pub fn assume_trusted_for_test() {
    ASSUME_TRUSTED.with(|c| c.set(true));
}

/// Trust the project containing `dir`. Returns the recorded root.
pub fn trust(home: &Path, dir: &Path) -> io::Result<PathBuf> {
    let root = project_root(dir);
    let mut list = trusted(home);
    if !list.contains(&root) {
        list.push(root.clone());
        write(home, &list)?;
    }
    Ok(root)
}

/// Stop trusting the project containing `dir`. `false` when it was not trusted.
pub fn untrust(home: &Path, dir: &Path) -> io::Result<bool> {
    let root = project_root(dir);
    let mut list = trusted(home);
    let before = list.len();
    list.retain(|p| p != &root);
    if list.len() == before {
        return Ok(false);
    }
    write(home, &list)?;
    Ok(true)
}

fn write(home: &Path, list: &[PathBuf]) -> io::Result<()> {
    std::fs::create_dir_all(home)?;
    let body = serde_json::to_string_pretty(list).map_err(io::Error::other)?;
    let tmp = home.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, home.join(FILE))
}

/// `gray trust [DIR]` and `gray trust --revoke [DIR]`.
pub fn run_cli(dir: Option<PathBuf>, revoke: bool) -> anyhow::Result<()> {
    let dir = match dir {
        Some(d) => d,
        None => std::env::current_dir()?,
    };
    let home = crate::skills::gray_agent_dir();
    let root = project_root(&dir);
    if revoke {
        if untrust(&home, &dir)? {
            println!("untrusted {}", root.display());
        } else {
            println!("not trusted: {}", root.display());
        }
    } else {
        trust(&home, &dir)?;
        println!(
            "trusted {} — its .claude/commands, .gray/prompts, SKILL.md and AGENTS.md now load",
            root.display()
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "project_trust_tests.rs"]
mod tests;
