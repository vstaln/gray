use std::path::PathBuf;

use crate::gray_home;

pub fn goal_path() -> anyhow::Result<PathBuf> {
    Ok(gray_home()?.join("heartbeat").join("goal.md"))
}

pub fn read_goal() -> anyhow::Result<String> {
    let path = goal_path()?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

pub fn write_goal(goal: &str) -> anyhow::Result<()> {
    let path = goal_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, goal)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_round_trips_and_is_empty_when_absent() {
        let _guard = crate::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
        assert_eq!(read_goal().unwrap(), "");
        write_goal("Ship the thing.\n").unwrap();
        assert_eq!(read_goal().unwrap(), "Ship the thing.\n");
    }
}
