use super::*;

impl GrayApp {
    pub(super) fn work_root(&self) -> Option<PathBuf> {
        if let Some(w) = self
            .active_thread
            .as_ref()
            .and_then(|t| self.store.state.thread(t))
            .and_then(|t| t.worktree.clone())
        {
            return Some(PathBuf::from(w));
        }
        self.project_path()
    }

    pub(super) fn project_path(&self) -> Option<PathBuf> {
        self.active_project
            .as_ref()
            .and_then(|id| self.store.state.project(id))
            .map(|p| PathBuf::from(&p.path))
    }

    pub(super) fn add_project(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let p = path.display().to_string();
        if let Some(existing) = self.store.state.projects.iter().find(|x| x.path == p) {
            self.active_project = Some(existing.id.clone());
            return;
        }
        let id = new_id();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| p.clone());
        self.persist(Event::ProjectAdded {
            id: id.clone(),
            name,
            path: p,
            at: now(),
        });
        self.active_project = Some(id);
        self.active_thread = None;
    }

    pub(super) fn refresh_workspace(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.work_root() else {
            self.tree.clear();
            self.branch = None;
            self.branches.clear();
            self.diff = None;
            self.diff_files.clear();
            return;
        };
        self.tree = workspace::tree(&root, &self.expanded);
        if git::is_repo(&root) {
            self.branch = git::branch(&root);
            self.branches = git::branches(&root);
            let text = match &self.diff_base {
                Some(base) => git::diff_since(&root, base),
                None => git::diff(&root),
            };
            let files = DiffFile::parse(&text).unwrap_or_default();
            self.diff_files = match &self.diff_base {
                Some(base) => git::files_since(&root, base),
                None => git::status(&root).into_iter().map(|(_, p)| p).collect(),
            };
            self.diff = if files.is_empty() {
                None
            } else {
                Some(cx.new(|cx| DiffState::new(files, cx)))
            };
        } else {
            self.branch = None;
            self.branches.clear();
            self.diff = None;
            self.diff_files.clear();
        }
    }
}
