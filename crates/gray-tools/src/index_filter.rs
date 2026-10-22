use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Find,
    Grep,
}

pub struct IgnoreChain {
    lane: Lane,

    git_root: Option<PathBuf>,

    global: Gitignore,

    cache: HashMap<PathBuf, Rc<Vec<Gitignore>>>,
}

impl IgnoreChain {
    pub fn new(root: &Path, lane: Lane) -> Self {
        let git_root = root
            .ancestors()
            .find(|p| p.join(".git").exists() || p.join(".jj").exists())
            .map(|p| p.to_path_buf());

        let global_at = git_root.clone().unwrap_or_else(|| root.to_path_buf());
        let global = GitignoreBuilder::new(&global_at).build_global().0;
        Self {
            lane,
            git_root,
            global,
            cache: HashMap::new(),
        }
    }

    pub fn is_ignored(&mut self, path: &Path, is_dir: bool) -> bool {
        let mut dirs: Vec<&Path> = path.ancestors().skip(1).collect();
        dirs.reverse();

        let mut seen: Vec<Rc<Vec<Gitignore>>> = Vec::with_capacity(dirs.len() + 1);
        if !self.global.is_empty() {
            seen.push(Rc::new(vec![self.global.clone()]));
        }
        for dir in dirs {
            if seen
                .iter()
                .flat_map(|v| v.iter())
                .any(|m| m.matched(dir, true).is_ignore())
            {
                return true;
            }
            seen.push(self.level(dir));
        }

        seen.iter()
            .flat_map(|v| v.iter())
            .rev()
            .find_map(|m| match m.matched(path, is_dir) {
                x if x.is_none() => None,
                x => Some(x.is_ignore()),
            })
            .unwrap_or(false)
    }

    fn level(&mut self, dir: &Path) -> Rc<Vec<Gitignore>> {
        if let Some(ms) = self.cache.get(dir) {
            return ms.clone();
        }
        let in_worktree = self.git_root.as_ref().is_some_and(|r| dir.starts_with(r));
        let mut files: Vec<PathBuf> = Vec::with_capacity(4);
        if self.git_root.as_deref() == Some(dir) {
            files.push(dir.join(".git/info/exclude"));
        }

        if self.lane == Lane::Find || in_worktree {
            files.push(dir.join(".gitignore"));
        }
        files.push(dir.join(".ignore"));
        files.push(dir.join(match self.lane {
            Lane::Find => ".fdignore",
            Lane::Grep => ".rgignore",
        }));

        let mut ms: Vec<Gitignore> = Vec::new();
        for file in files {
            let mut b = GitignoreBuilder::new(dir);

            if b.add(&file).is_none()
                && let Ok(g) = b.build()
                && !g.is_empty()
            {
                ms.push(g);
            }
        }
        let ms = Rc::new(ms);
        self.cache.insert(dir.to_path_buf(), ms.clone());
        ms
    }
}
