use std::io;
use std::path::{Path, PathBuf};

const FILE: &str = "trusted_projects.json";

pub(crate) fn find_git_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

#[must_use]
pub fn project_root(dir: &Path) -> PathBuf {
    let base = find_git_root(dir).unwrap_or_else(|| dir.to_path_buf());
    std::fs::canonicalize(&base).unwrap_or(base)
}

#[must_use]
pub fn trusted(home: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(home.join(FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<PathBuf>>(&raw).ok())
        .unwrap_or_default()
}

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

#[cfg(test)]
pub fn assume_trusted_for_test() {
    ASSUME_TRUSTED.with(|c| c.set(true));
}

pub fn trust(home: &Path, dir: &Path) -> io::Result<PathBuf> {
    let root = project_root(dir);
    let mut list = trusted(home);
    if !list.contains(&root) {
        list.push(root.clone());
        write(home, &list)?;
    }
    Ok(root)
}

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

pub fn run_cli(dir: Option<PathBuf>, revoke: bool) -> anyhow::Result<()> {
    let dir = match dir {
        Some(d) => d,
        None => std::env::current_dir()?,
    };
    let home = gray_core::paths::gray_home_or_temp();
    let root = project_root(&dir);
    if revoke {
        if untrust(&home, &dir)? {
            println!("untrusted {}", root.display());
        } else {
            println!("not trusted: {}", root.display());
        }
    } else {
        trust(&home, &dir)?;
        println!("trusted {} — its .gray/prompts now load", root.display());
    }
    Ok(())
}

#[cfg(test)]
#[path = "project_trust_tests.rs"]
mod tests;
