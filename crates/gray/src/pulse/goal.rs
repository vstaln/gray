use std::path::{Path, PathBuf};

use gray_gateway::config::gray_home_dir;

pub(crate) fn goal_path_at(home: &Path) -> PathBuf {
    home.join("pulse").join("goal.md")
}

pub fn goal_path() -> anyhow::Result<PathBuf> {
    Ok(goal_path_at(&gray_home_dir()?))
}

pub(crate) fn read_goal_at(path: &Path) -> anyhow::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

pub fn read_goal() -> anyhow::Result<String> {
    read_goal_at(&goal_path()?)
}

pub(crate) fn write_goal_at(path: &Path, goal: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, goal)?;
    Ok(())
}

pub fn write_goal(goal: &str) -> anyhow::Result<()> {
    write_goal_at(&goal_path()?, goal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_round_trips_and_is_empty_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = goal_path_at(dir.path());
        assert_eq!(read_goal_at(&path).unwrap(), "");
        write_goal_at(&path, "Ship the thing.\n").unwrap();
        assert_eq!(read_goal_at(&path).unwrap(), "Ship the thing.\n");
    }
}
