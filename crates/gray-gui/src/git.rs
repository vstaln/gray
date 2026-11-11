use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        None
    }
}

pub fn is_repo(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--is-inside-work-tree"]).is_some()
}

pub fn branch(dir: &Path) -> Option<String> {
    git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn branches(dir: &Path) -> Vec<String> {
    git(dir, &["branch", "--format=%(refname:short)"])
        .map(|s| {
            s.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub fn checkout(dir: &Path, name: &str, create: bool) -> Result<(), String> {
    let mut args = vec!["checkout"];
    if create {
        args.push("-b");
    }
    args.push(name);
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(&args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn status(dir: &Path) -> Vec<(String, String)> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain=v1", "-z", "-uall"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string());
    let Some(out) = out else {
        return Vec::new();
    };
    let mut recs = out.split('\0');
    let mut res = Vec::new();
    while let Some(r) = recs.next() {
        if r.len() <= 3 {
            continue;
        }
        let code = r[..2].trim().to_string();
        if code.contains('R') || code.contains('C') {
            let _ = recs.next();
        }
        res.push((code, r[3..].to_string()));
    }
    res
}

pub fn diff(dir: &Path) -> String {
    let mut text = git(dir, &["diff", "HEAD", "--no-color"])
        .or_else(|| git(dir, &["diff", "--no-color"]))
        .unwrap_or_default();
    for (code, path) in status(dir) {
        if code == "??"
            && let Ok(d) = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["diff", "--no-color", "--no-index", "/dev/null", &path])
                .output()
        {
            text.push_str(&String::from_utf8_lossy(&d.stdout));
        }
    }
    text
}

pub fn diff_since(dir: &Path, base: &str) -> String {
    let mut text = git(dir, &["diff", base, "--no-color"]).unwrap_or_default();
    for (code, path) in status(dir) {
        if code == "??"
            && let Ok(d) = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["diff", "--no-color", "--no-index", "/dev/null", &path])
                .output()
        {
            text.push_str(&String::from_utf8_lossy(&d.stdout));
        }
    }
    text
}

pub fn files_since(dir: &Path, base: &str) -> Vec<String> {
    let mut v: Vec<String> = git(dir, &["diff", "--name-only", base])
        .map(|t| t.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default();
    for (code, path) in status(dir) {
        if code == "??" && !v.contains(&path) {
            v.push(path);
        }
    }
    v
}

pub fn worktree_add(root: &Path, dir: &Path, branch: &str) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["worktree", "add", "-b", branch])
        .arg(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn checkpoint(dir: &Path) -> Option<String> {
    let index = std::env::temp_dir().join(format!("gray-gui-index-{}", std::process::id()));
    let _ = std::fs::remove_file(&index);
    let run = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .env("GIT_INDEX_FILE", &index)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    };
    let head = run(&["rev-parse", "--verify", "HEAD^{tree}"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(tree) = &head {
        run(&["read-tree", tree])?;
    }
    run(&["add", "-A"])?;
    let tree = run(&["write-tree"]).map(|s| s.trim().to_string());
    let _ = std::fs::remove_file(&index);
    tree
}

pub fn restore(dir: &Path, tree: &str) -> Result<(), String> {
    let run = |args: &[&str]| -> Result<(), String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    };
    run(&["add", "-A"])?;
    run(&["read-tree", "-u", "--reset", tree])?;
    run(&["reset", "-q"])
}
