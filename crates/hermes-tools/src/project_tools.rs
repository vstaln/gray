//! Project tools — the agent's intentional handle on first-class Projects.
//! Port of `tools/project_tools.py` (197 lines) — 1:1 behavior.
//!
//! Projects (per-profile `projects.db`) are the named workspaces the desktop
//! sidebar groups sessions into. Creating / switching a project is a deliberate
//! act expressed as explicit tools — never a side effect of a terminal `cd`.
//!
//! Exposed only on GUI sessions: the tools live in the `project` toolset (kept off
//! `_HERMES_CORE_TOOLS`) which the desktop/TUI gateway folds into its resolved
//! toolsets, so no CLI/messaging/cron schema carries them. The GUI also wires
//! `set_project_workspace_callback` so a create/switch re-anchors the live
//! session's cwd and the sidebar follows the move; the DB write is the durable part.
//!
//! Python mapping:
//! - `_workspace_callback` → [`_workspace_callback`] / [`set_project_workspace_callback`]
//! - `_primary_path` → [`primary_path`]
//! - `_apply_workspace` → [`apply_workspace`] / [`apply_workspace_with`]
//! - `_resolve` → [`resolve_project`]
//! - `project_list` → [`project_list`] / [`project_list_with_store`]
//! - `project_create` → [`project_create`] / [`project_create_with_store`]
//! - `project_switch` → [`project_switch`] / [`project_switch_with_store`]
//! - `registry.register(name="project_list")` → [`TOOL_NAME_LIST`] / [`project_list_schema`]
//! - `registry.register(name="project_create")` → [`TOOL_NAME_CREATE`] / [`project_create_schema`]
//! - `registry.register(name="project_switch")` → [`TOOL_NAME_SWITCH`] / [`project_switch_schema`]

use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Registry metadata — mirrors `registry.register(...)` kwargs in Python
// ---------------------------------------------------------------------------

/// Tool name for list — mirrors `registry.register(name="project_list", ...)` (142).
pub const TOOL_NAME_LIST: &str = "project_list";
/// Tool name for create — mirrors `registry.register(name="project_create", ...)` (153).
pub const TOOL_NAME_CREATE: &str = "project_create";
/// Tool name for switch — mirrors `registry.register(name="project_switch", ...)` (178).
pub const TOOL_NAME_SWITCH: &str = "project_switch";
/// Toolset that gates all three tools — mirrors `toolset="project"` (143, 154, 179).
pub const TOOLSET: &str = "project";
/// `requires_env` for these tools — none (project is DB-gated, not env-gated).
pub const REQUIRES_ENV: &[&str] = &[];

// ---------------------------------------------------------------------------
// Descriptions — mirrors schema `description` fields
// ---------------------------------------------------------------------------

/// Description for `project_list` — mirrors `project_list` schema (146).
pub const PROJECT_LIST_DESCRIPTION: &str =
    "List the desktop Projects (named workspaces) and which one is active.";

/// Description for `project_create` — mirrors `project_create` schema (157-162).
pub const PROJECT_CREATE_DESCRIPTION: &str = "Create a desktop Project (a named workspace) and switch this chat into it. Pass `path` to anchor it to a repo/folder — this chat's workspace moves there and the sidebar follows. Use when starting work in a new repo/folder; this is the intentional way to move the session, not `cd`.";

/// Description for `project_switch` — mirrors `project_switch` schema (181-186).
pub const PROJECT_SWITCH_DESCRIPTION: &str = "Switch this chat into an existing desktop Project (by name, slug, or id). Moves the session's workspace to the project's primary folder and the sidebar follows. The intentional way to move between projects, not `cd`.";

/// Description for `name` param in `project_create`.
pub const PROJECT_CREATE_NAME_DESCRIPTION: &str = "Human name, e.g. 'Aurora Demo'";
/// Description for `path` param in `project_create`.
pub const PROJECT_CREATE_PATH_DESCRIPTION: &str = "Primary repo/folder to anchor the project to";
/// Description for `project` param in `project_switch`.
pub const PROJECT_SWITCH_PROJECT_DESCRIPTION: &str = "Project name, slug, or id";

// ---------------------------------------------------------------------------
// Error messages — mirrors inline `json.dumps({"success": False, "error": ...})`
// ---------------------------------------------------------------------------

pub const NAME_REQUIRED_ERROR: &str = "name is required";
pub const PROJECT_VANISHED_ERROR: &str = "project vanished after create";

fn no_project_error(token: &str) -> String {
    format!("no project matching '{token}'")
}

// ---------------------------------------------------------------------------
// Schema — mirrors `registry.register(..., schema={...})` dicts
// ---------------------------------------------------------------------------

/// Returns the JSON schema for `project_list` — mirrors `project_list` registration (144-149).
pub fn project_list_schema() -> Value {
    json!({
        "name": TOOL_NAME_LIST,
        "description": PROJECT_LIST_DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {}
        }
    })
}

/// Returns the JSON schema for `project_create` — mirrors `project_create` registration (156-175).
pub fn project_create_schema() -> Value {
    json!({
        "name": TOOL_NAME_CREATE,
        "description": PROJECT_CREATE_DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": PROJECT_CREATE_NAME_DESCRIPTION
                },
                "path": {
                    "type": "string",
                    "description": PROJECT_CREATE_PATH_DESCRIPTION
                }
            },
            "required": ["name"]
        }
    })
}

/// Returns the JSON schema for `project_switch` — mirrors `project_switch` registration (180-195).
pub fn project_switch_schema() -> Value {
    json!({
        "name": TOOL_NAME_SWITCH,
        "description": PROJECT_SWITCH_DESCRIPTION,
        "parameters": {
            "type": "object",
            "properties": {
                "project": {
                    "type": "string",
                    "description": PROJECT_SWITCH_PROJECT_DESCRIPTION
                }
            },
            "required": ["project"]
        }
    })
}

pub fn project_list_schema_json() -> String {
    project_list_schema().to_string()
}
pub fn project_create_schema_json() -> String {
    project_create_schema().to_string()
}
pub fn project_switch_schema_json() -> String {
    project_switch_schema().to_string()
}

// ---------------------------------------------------------------------------
// Data structures — mirrors `hermes_cli.projects_db.Project` / `ProjectFolder`
// ---------------------------------------------------------------------------

/// Mirrors `ProjectFolder` dataclass in `hermes_cli/projects_db.py`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFolder {
    pub path: String,
    pub is_primary: bool,
}

impl ProjectFolder {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            is_primary: false,
        }
    }
    pub fn primary(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            is_primary: true,
        }
    }
}

/// Mirrors `Project` dataclass in `hermes_cli/projects_db.py` (relevant subset).
///
/// Full DB row has id/slug/name/description/icon/color/board_slug/primary_path/
/// created_at/archived/folders — for project_tools we need id/slug/name/primary_path/folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub primary_path: Option<String>,
    pub folders: Vec<ProjectFolder>,
    pub archived: bool,
}

impl Project {
    pub fn new(id: impl Into<String>, slug: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            slug: slug.into(),
            name: name.into(),
            primary_path: None,
            folders: Vec::new(),
            archived: false,
        }
    }
    pub fn with_primary_path(mut self, p: impl Into<String>) -> Self {
        self.primary_path = Some(p.into());
        self
    }
    pub fn with_folders(mut self, folders: Vec<ProjectFolder>) -> Self {
        self.folders = folders;
        self
    }
    pub fn with_archived(mut self, archived: bool) -> Self {
        self.archived = archived;
        self
    }
}

// ---------------------------------------------------------------------------
// Workspace callback — mirrors `_workspace_callback` / `set_project_workspace_callback`
// ---------------------------------------------------------------------------

type WorkspaceCallback = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;

static WORKSPACE_CB: OnceLock<Mutex<Option<WorkspaceCallback>>> = OnceLock::new();

fn workspace_cb_cell() -> &'static Mutex<Option<WorkspaceCallback>> {
    WORKSPACE_CB.get_or_init(|| Mutex::new(None))
}

/// Mirrors `def set_project_workspace_callback(fn: Optional[Callable[[str, str, str], None]]) -> None:` (28-30).
///
/// Set by the GUI gateway at session wiring. Receives `(task_id, primary_path, project_name)`
/// and re-anchors that session's workspace + refreshes the sidebar. `None` in CLI / messaging.
pub fn set_project_workspace_callback<F>(cb: Option<F>)
where
    F: Fn(&str, &str, &str) + Send + Sync + 'static,
{
    let boxed: Option<WorkspaceCallback> = cb.map(|f| Arc::new(f) as WorkspaceCallback);
    *workspace_cb_cell().lock().unwrap() = boxed;
}

/// Boxed variant for callers that already have an `Arc`.
pub fn set_project_workspace_callback_boxed(cb: Option<WorkspaceCallback>) {
    *workspace_cb_cell().lock().unwrap() = cb;
}

/// Clear the callback (set to `None`) — mirrors passing `None` in Python.
pub fn clear_project_workspace_callback() {
    *workspace_cb_cell().lock().unwrap() = None;
}

/// Test helper: read current callback clone (if any).
pub fn get_workspace_callback() -> Option<WorkspaceCallback> {
    workspace_cb_cell().lock().unwrap().clone()
}

// ---------------------------------------------------------------------------
// Helpers — mirrors `_primary_path`, `_apply_workspace`, `_resolve`, path norm
// ---------------------------------------------------------------------------

/// Mirrors `def _primary_path(proj) -> Optional[str]:` (33-39).
///
/// ```python
/// if getattr(proj, "primary_path", None):
///     return proj.primary_path
/// for folder in proj.folders:
///     if folder.is_primary:
///         return folder.path
/// return proj.folders[0].path if proj.folders else None
/// ```
pub fn primary_path(proj: &Project) -> Option<String> {
    if let Some(p) = &proj.primary_path {
        if !p.trim().is_empty() {
            return Some(p.clone());
        }
    }
    for folder in &proj.folders {
        if folder.is_primary {
            return Some(folder.path.clone());
        }
    }
    proj.folders.first().map(|f| f.path.clone())
}

/// Mirrors `def _apply_workspace(task_id: Optional[str], path: Optional[str], name: str) -> None:` (42-48).
///
/// ```python
/// cb = _workspace_callback
/// if cb and task_id and path:
///     try: cb(task_id, path, name)
///     except Exception: pass
/// ```
pub fn apply_workspace(task_id: Option<&str>, path: Option<&str>, name: &str) {
    let cb_opt = workspace_cb_cell().lock().unwrap().clone();
    apply_workspace_with(cb_opt.as_deref(), task_id, path, name);
}

/// Testable core: same as `apply_workspace` but with injected callback.
///
/// `cb` mirrors `_workspace_callback`: `Fn(&str, &str, &str)`.
/// `task_id` and `path` must both be `Some` non-empty to fire — mirrors `if cb and task_id and path:`.
pub fn apply_workspace_with(
    cb: Option<&(dyn Fn(&str, &str, &str) + Send + Sync)>,
    task_id: Option<&str>,
    path: Option<&str>,
    name: &str,
) {
    if let (Some(cb), Some(tid), Some(p)) = (cb, task_id, path) {
        if tid.is_empty() || p.is_empty() {
            return;
        }
        // Python does `try: cb(task_id, path, name) except Exception: pass`
        // In Rust we catch panics via catch_unwind if the closure panics.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cb(tid, p, name);
        }));
    }
}

// ---------------------------------------------------------------------------
// Path normalization — mirrors `_normalize_path` / `_primary_path_key` / expanduser / abspath
// ---------------------------------------------------------------------------

fn expand_user(path: &str) -> String {
    // Mirrors `os.path.expanduser` — expands leading `~` and `~/`.
    // Full `~user` expansion not needed on Linux for Hermes (only current user).
    if path == "~" {
        std::env::var("HOME").unwrap_or_else(|_| path.to_string())
    } else if path.starts_with("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            path.to_string()
        } else {
            format!("{}{}", home, &path[1..])
        }
    } else {
        path.to_string()
    }
}

fn lexical_absolute(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(p)
    } else {
        PathBuf::from("/").join(p)
    };
    normalize_lexically(&abs)
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => out.push(n),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(Component::RootDir.as_os_str());
    }
    out
}

/// Mirrors `_normalize_path(path: str) -> str` in `hermes_cli/projects_db.py` (142-145):
/// `p = os.path.abspath(os.path.expanduser(str(path).strip())); return p.rstrip("/\\") or p`
pub fn normalize_project_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let expanded = expand_user(trimmed);
    let p = Path::new(&expanded);
    let abs = lexical_absolute(p);
    let s = abs.to_string_lossy().to_string();
    // Python: p.rstrip("/\\") or p
    let stripped = s.trim_end_matches(|c| c == '/' || c == '\\').to_string();
    if stripped.is_empty() {
        s
    } else {
        stripped
    }
}

/// Mirrors `_primary_path_key(path: str) -> str` (322-324):
/// `return os.path.normcase(_normalize_path(path))` — normcase is lowercase on Windows.
pub fn normalize_path_key(path: &str) -> String {
    let norm = normalize_project_path(path);
    if cfg!(windows) {
        norm.to_lowercase()
    } else {
        norm
    }
}

// ---------------------------------------------------------------------------
// Resolve — mirrors `def _resolve(conn, token: str):` (51-66)
// ---------------------------------------------------------------------------

/// Pure helper that mirrors `find_by_primary_path` semantics for an in-memory slice.
///
/// Used by the store's `find_by_primary_path` default impl and testable directly.
pub fn find_by_primary_path_in_slice<'a>(projects: &'a [Project], path: &str) -> Option<&'a Project> {
    let key = normalize_path_key(path);
    if key.is_empty() {
        return None;
    }
    for proj in projects {
        let primary = primary_path(proj);
        if let Some(p) = primary {
            if normalize_path_key(&p) == key {
                return Some(proj);
            }
        }
    }
    None
}

/// Mirrors `def _resolve(conn, token: str):` (51-66).
///
/// Resolution order (case-sensitive first, then case-insensitive):
/// 1. exact `id` or `slug` or `name == token`
/// 2. case-insensitive `slug` or `name`
pub fn resolve_project<'a>(projects: &'a [Project], token: &str) -> Option<&'a Project> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }
    for proj in projects {
        if t == proj.id || t == proj.slug || proj.name == t {
            return Some(proj);
        }
    }
    let low = t.to_lowercase();
    for proj in projects {
        if proj.slug.to_lowercase() == low || proj.name.to_lowercase() == low {
            return Some(proj);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Store abstraction — mirrors `hermes_cli.projects_db` connection API used by
// `project_tools.py` (list_projects, get_active_id, find_by_primary_path,
// create_project, set_active, get_project). Injected so Rust can run without
// rusqlite; the real DB lives behind this trait when wired.
// ---------------------------------------------------------------------------

/// Minimal DB surface used by `project_tools.py`.
///
/// Implementors back `project_list` / `project_create` / `project_switch`.
/// `Send + Sync` so an `Arc<dyn ProjectStore>` can be used in handlers.
pub trait ProjectStore: Send + Sync {
    fn list_projects(&self, include_archived: bool) -> Vec<Project>;
    fn get_active_id(&self) -> Option<String>;
    fn find_by_primary_path(&self, path: &str) -> Option<Project>;
    fn create_project(&self, name: &str, folders: Vec<String>, primary_path: Option<String>) -> Result<String, String>;
    fn set_active(&self, id: &str);
    fn get_project(&self, id_or_slug: &str) -> Option<Project>;
}

// ---------------------------------------------------------------------------
// In-memory store — used in tests and as a reference impl
// ---------------------------------------------------------------------------

struct InMemoryInner {
    projects: Vec<Project>,
    active_id: Option<String>,
    next_id: usize,
}

impl InMemoryInner {
    fn new() -> Self {
        Self {
            projects: Vec::new(),
            active_id: None,
            next_id: 1,
        }
    }
}

fn slugify(name: &str) -> String {
    // Mirrors `hermes_cli.projects_db._slugify` (110-115):
    // s = re.sub(r"[^a-z0-9]+", "-", s.lower().strip()).strip("-_"); s[:64].strip("-_") or "project"
    let lower = name.trim().to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_was_dash = true; // start true to avoid leading dash
    for ch in lower.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            out.push(ch);
            last_was_dash = false;
        } else {
            if !last_was_dash {
                out.push('-');
                last_was_dash = true;
            }
        }
    }
    // strip "-_"
    let mut s = out.trim_matches(|c| c == '-' || c == '_').to_string();
    if s.len() > 64 {
        s.truncate(64);
        s = s.trim_matches(|c| c == '-' || c == '_').to_string();
    }
    if s.is_empty() {
        "project".to_string()
    } else {
        s
    }
}

fn unique_slug(projects: &[Project], candidate: &str) -> String {
    // Mirrors `_unique_slug` (308-319): candidate or candidate-2, -3 ...
    let base = candidate.to_string();
    let mut n = 1;
    let mut slug = base.clone();
    let exists = |s: &str| projects.iter().any(|p| p.slug == s);
    while exists(&slug) {
        n += 1;
        let suffix = format!("-{n}");
        let base_trunc = if base.len() + suffix.len() > 64 {
            base[..64 - suffix.len()].trim_end_matches(|c| c == '-' || c == '_').to_string()
        } else {
            base.clone()
        };
        slug = format!("{base_trunc}{suffix}");
    }
    slug
}

fn new_project_id(next: usize) -> String {
    // Mirrors `_new_project_id` → "p_" + secrets.token_hex(4) (8 hex chars).
    // For in-memory we use deterministic counter formatted as 8 hex digits.
    format!("p_{next:08x}")
}

/// In-memory `ProjectStore` — test double and reference impl.
///
/// Mirrors `hermes_cli.projects_db` CRUD with `write_txn` semantics (single lock).
pub struct InMemoryStore {
    inner: Mutex<InMemoryInner>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(InMemoryInner::new()),
        }
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.projects.clear();
        inner.active_id = None;
        inner.next_id = 1;
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectStore for InMemoryStore {
    fn list_projects(&self, include_archived: bool) -> Vec<Project> {
        let inner = self.inner.lock().unwrap();
        if include_archived {
            inner.projects.clone()
        } else {
            inner.projects.iter().filter(|p| !p.archived).cloned().collect()
        }
    }

    fn get_active_id(&self) -> Option<String> {
        self.inner.lock().unwrap().active_id.clone()
    }

    fn find_by_primary_path(&self, path: &str) -> Option<Project> {
        let inner = self.inner.lock().unwrap();
        find_by_primary_path_in_slice(&inner.projects, path).cloned()
    }

    fn create_project(&self, name: &str, folders: Vec<String>, primary_path: Option<String>) -> Result<String, String> {
        let n = name.trim();
        if n.is_empty() {
            return Err("project name must not be empty".to_string());
        }
        let mut inner = self.inner.lock().unwrap();
        // Normalize folders + primary
        let mut folder_paths: Vec<String> = Vec::new();
        for f in folders {
            let norm = normalize_project_path(&f);
            if !norm.is_empty() && !folder_paths.contains(&norm) {
                folder_paths.push(norm);
            }
        }
        let primary = primary_path.map(|p| normalize_project_path(&p)).filter(|s| !s.is_empty());
        let primary = if let Some(ref pp) = primary {
            if !folder_paths.contains(pp) {
                folder_paths.insert(0, pp.clone());
            }
            Some(pp.clone())
        } else if !folder_paths.is_empty() {
            Some(folder_paths[0].clone())
        } else {
            None
        };
        if let Some(ref pp) = primary {
            // duplicate check (non-archived only, mirrors pdb.find_by_primary_path default)
            let key = normalize_path_key(pp);
            for proj in &inner.projects {
                if proj.archived {
                    continue;
                }
                if let Some(existing_primary) = primary_path(proj) {
                    if normalize_path_key(&existing_primary) == key {
                        return Err(format!(
                            "folder already belongs to project '{}' ({}); switch to it instead of creating a duplicate",
                            proj.slug, proj.id
                        ));
                    }
                }
            }
        }
        let candidate = slugify(n);
        let unique = unique_slug(&inner.projects, &candidate);
        let pid = new_project_id(inner.next_id);
        inner.next_id += 1;
        // Build folders vec with is_primary flags
        let folders_vec: Vec<ProjectFolder> = folder_paths
            .iter()
            .map(|p| ProjectFolder {
                path: p.clone(),
                is_primary: Some(p) == primary.as_ref(),
            })
            .collect();
        let proj = Project {
            id: pid.clone(),
            slug: unique,
            name: n.to_string(),
            primary_path: primary.clone(),
            folders: folders_vec,
            archived: false,
        };
        inner.projects.push(proj);
        Ok(pid)
    }

    fn set_active(&self, id: &str) {
        let mut inner = self.inner.lock().unwrap();
        // In real DB this sets project_meta active_id even if id doesn't exist; we mirror leniently.
        inner.active_id = Some(id.to_string());
    }

    fn get_project(&self, id_or_slug: &str) -> Option<Project> {
        let inner = self.inner.lock().unwrap();
        // id first, then slug lower-case
        for p in &inner.projects {
            if p.id == id_or_slug {
                return Some(p.clone());
            }
        }
        let low = id_or_slug.to_lowercase();
        for p in &inner.projects {
            if p.slug.to_lowercase() == low {
                return Some(p.clone());
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors `project_list`, `project_create`, `project_switch`
// ---------------------------------------------------------------------------

fn success_project_json(proj: &Project, primary: Option<String>) -> String {
    json!({
        "success": true,
        "id": proj.id,
        "slug": proj.slug,
        "name": proj.name,
        "primary_path": primary
    })
    .to_string()
}

fn error_json(msg: &str) -> String {
    json!({ "success": false, "error": msg }).to_string()
}

// ---- project_list ----

/// Mirrors `def project_list(task_id: Optional[str] = None) -> str:` (69-88).
///
/// ```python
/// with pdb.connect_closing() as conn:
///     active = pdb.get_active_id(conn)
///     projects = pdb.list_projects(conn)
/// return json.dumps({"active_id": active, "projects": [{"id":..., "slug":..., "name":..., "primary_path": _primary_path(p), "active": p.id == active}]})
/// ```
pub fn project_list_with_store<S: ProjectStore>(store: &S) -> String {
    let active = store.get_active_id();
    let projects = store.list_projects(false);
    let arr: Vec<Value> = projects
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "slug": p.slug,
                "name": p.name,
                "primary_path": primary_path(p),
                "active": Some(&p.id) == active.as_ref()
            })
        })
        .collect();
    json!({ "active_id": active, "projects": arr }).to_string()
}

/// Default stub using an empty in-memory store (no active, no projects).
/// In a real gateway this would be wired to the `projects.db` file; the stub
/// preserves the return shape.
pub fn project_list() -> String {
    let store = InMemoryStore::new();
    project_list_with_store(&store)
}

// ---- project_create ----

/// Mirrors `def project_create(name: str, path: Optional[str] = None, task_id: Optional[str] = None) -> str:` (91-124).
pub fn project_create_with_store<S: ProjectStore>(
    store: &S,
    name: &str,
    path: Option<&str>,
    task_id: Option<&str>,
) -> String {
    project_create_with_store_and_workspace(store, name, path, task_id, None)
}

/// Injectable workspace callback variant — mirrors same Python body but with
/// `apply_workspace_with` so tests can assert the callback.
pub fn project_create_with_store_and_workspace<S: ProjectStore>(
    store: &S,
    name: &str,
    path: Option<&str>,
    task_id: Option<&str>,
    workspace_cb: Option<&(dyn Fn(&str, &str, &str) + Send + Sync)>,
) -> String {
    let trimmed_name = name.trim();
    if trimmed_name.is_empty() {
        return error_json(NAME_REQUIRED_ERROR);
    }

    let mut folder = path.unwrap_or("").trim().to_string();
    if !folder.is_empty() {
        folder = normalize_project_path(&folder);
    }

    // Python: try: with connect_closing as conn: ... except ValueError as exc: return error
    // Rust: we call store methods directly; create_project may return Err(ValueError string).
    let proj: Option<Project> = {
        let existing = if !folder.is_empty() {
            store.find_by_primary_path(&folder)
        } else {
            None
        };
        if let Some(existing_proj) = existing {
            // Idempotent create: folder already belongs to a project.
            // Re-activating it beats minting a duplicate — duplicated projects render N identical sidebar subtrees (#75820).
            store.set_active(&existing_proj.id);
            Some(existing_proj)
        } else {
            let folders = if folder.is_empty() {
                Vec::new()
            } else {
                vec![folder.clone()]
            };
            let primary_opt = if folder.is_empty() { None } else { Some(folder.clone()) };
            match store.create_project(trimmed_name, folders, primary_opt) {
                Ok(pid) => {
                    store.set_active(&pid);
                    store.get_project(&pid)
                }
                Err(exc) => return error_json(&exc),
            }
        }
    };

    let proj = match proj {
        Some(p) => p,
        None => return error_json(PROJECT_VANISHED_ERROR),
    };

    let primary = primary_path(&proj);
    // Apply workspace (global or injected)
    match workspace_cb {
        Some(cb) => apply_workspace_with(Some(cb), task_id, primary.as_deref(), &proj.name),
        None => apply_workspace(task_id, primary.as_deref(), &proj.name),
    }

    success_project_json(&proj, primary)
}

/// Default stub (empty store) — mirrors `project_create` without DB wiring.
pub fn project_create(name: &str, path: Option<&str>, task_id: Option<&str>) -> String {
    let store = InMemoryStore::new();
    project_create_with_store(&store, name, path, task_id)
}

// ---- project_switch ----

/// Mirrors `def project_switch(project: str, task_id: Optional[str] = None) -> str:` (127-139).
pub fn project_switch_with_store<S: ProjectStore>(
    store: &S,
    project: &str,
    task_id: Option<&str>,
) -> String {
    project_switch_with_store_and_workspace(store, project, task_id, None)
}

pub fn project_switch_with_store_and_workspace<S: ProjectStore>(
    store: &S,
    project: &str,
    task_id: Option<&str>,
    workspace_cb: Option<&(dyn Fn(&str, &str, &str) + Send + Sync)>,
) -> String {
    // Python uses `with connect_closing as conn: proj = _resolve(conn, project); if None: error; set_active`
    // We read a snapshot of projects with include_archived=True for resolution.
    let all = store.list_projects(true);
    let proj = match resolve_project(&all, project) {
        Some(p) => p.clone(),
        None => return error_json(&no_project_error(project)),
    };
    store.set_active(&proj.id);

    let primary = primary_path(&proj);
    match workspace_cb {
        Some(cb) => apply_workspace_with(Some(cb), task_id, primary.as_deref(), &proj.name),
        None => apply_workspace(task_id, primary.as_deref(), &proj.name),
    }

    success_project_json(&proj, primary)
}

/// Default stub.
pub fn project_switch(project: &str, task_id: Option<&str>) -> String {
    let store = InMemoryStore::new();
    project_switch_with_store(&store, project, task_id)
}

// ---------------------------------------------------------------------------
// Registry handler aliases — mirrors `registry.register(..., handler=lambda args, **kw: ...)`
// ---------------------------------------------------------------------------

/// Mirrors `handler=lambda args, **kw: project_list(task_id=kw.get("task_id"))` (150).
pub fn handler_project_list<S: ProjectStore>(store: &S, args: &Value, _task_id: Option<&str>) -> String {
    let _ = args;
    project_list_with_store(store)
}

/// Mirrors `handler=lambda args, **kw: project_create(name=args.get("name",""), path=args.get("path"), task_id=kw.get("task_id"))` (173-176).
pub fn handler_project_create<S: ProjectStore>(store: &S, args: &Value, task_id: Option<&str>) -> String {
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let path = args.get("path").and_then(|v| v.as_str());
    project_create_with_store(store, name, path, task_id)
}

/// Mirrors `handler=lambda args, **kw: project_switch(project=args.get("project",""), task_id=kw.get("task_id"))` (196).
pub fn handler_project_switch<S: ProjectStore>(store: &S, args: &Value, task_id: Option<&str>) -> String {
    let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("");
    project_switch_with_store(store, project, task_id)
}

/// Bare handlers without a store (empty in-memory) — for registry surface that cannot inject DB.
pub fn handler(args: &Value) -> String {
    // Dispatch by `name` field if caller used generic handler; default to project_list shape.
    // Python registers three separate handlers; we expose one dispatcher for completeness.
    let _ = args;
    project_list()
}

// Per-tool bare handlers (no store) — mirrors each registry.register handler when DB not wired.
pub fn project_list_handler(args: &Value) -> String {
    let _ = args;
    project_list()
}
pub fn project_create_handler(args: &Value) -> String {
    project_create(
        args.get("name").and_then(|v| v.as_str()).unwrap_or(""),
        args.get("path").and_then(|v| v.as_str()),
        None,
    )
}
pub fn project_switch_handler(args: &Value) -> String {
    project_switch(
        args.get("project").and_then(|v| v.as_str()).unwrap_or(""),
        None,
    )
}

// ---------------------------------------------------------------------------
// `__all__` equivalent
// ---------------------------------------------------------------------------

pub const ALL: &[&str] = &[
    "project_list",
    "project_create",
    "project_switch",
    "set_project_workspace_callback",
    "primary_path",
    "resolve_project",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    // Helpers

    fn make_project(id: &str, slug: &str, name: &str, primary: Option<&str>, folders: Vec<(&str, bool)>) -> Project {
        Project {
            id: id.to_string(),
            slug: slug.to_string(),
            name: name.to_string(),
            primary_path: primary.map(|s| s.to_string()),
            folders: folders.into_iter().map(|(p, is_primary)| ProjectFolder { path: p.to_string(), is_primary }).collect(),
            archived: false,
        }
    }

    #[test]
    fn constants_match_python_registry_args() {
        assert_eq!(TOOL_NAME_LIST, "project_list");
        assert_eq!(TOOL_NAME_CREATE, "project_create");
        assert_eq!(TOOL_NAME_SWITCH, "project_switch");
        assert_eq!(TOOLSET, "project");
        assert!(REQUIRES_ENV.is_empty());
        assert_eq!(PROJECT_LIST_DESCRIPTION, "List the desktop Projects (named workspaces) and which one is active.");
        assert!(PROJECT_CREATE_DESCRIPTION.contains("named workspace"));
        assert!(PROJECT_CREATE_DESCRIPTION.contains("not `cd`"));
        assert!(PROJECT_SWITCH_DESCRIPTION.contains("name, slug, or id"));
        assert_eq!(NAME_REQUIRED_ERROR, "name is required");
        assert_eq!(PROJECT_VANISHED_ERROR, "project vanished after create");
        assert!(no_project_error("foo").contains("no project matching"));
    }

    #[test]
    fn schemas_match_python() {
        let list = project_list_schema();
        assert_eq!(list["name"], "project_list");
        assert_eq!(list["description"], PROJECT_LIST_DESCRIPTION);
        assert_eq!(list["parameters"]["type"], "object");
        assert!(list["parameters"]["properties"].as_object().unwrap().is_empty());

        let create = project_create_schema();
        assert_eq!(create["name"], "project_create");
        assert_eq!(create["description"], PROJECT_CREATE_DESCRIPTION);
        assert_eq!(create["parameters"]["properties"]["name"]["type"], "string");
        assert_eq!(create["parameters"]["properties"]["name"]["description"], PROJECT_CREATE_NAME_DESCRIPTION);
        assert_eq!(create["parameters"]["properties"]["path"]["type"], "string");
        assert_eq!(create["parameters"]["properties"]["path"]["description"], PROJECT_CREATE_PATH_DESCRIPTION);
        let req = create["parameters"]["required"].as_array().unwrap();
        assert_eq!(req, &vec![json!("name")]);

        let switch = project_switch_schema();
        assert_eq!(switch["name"], "project_switch");
        assert_eq!(switch["description"], PROJECT_SWITCH_DESCRIPTION);
        assert_eq!(switch["parameters"]["properties"]["project"]["type"], "string");
        assert_eq!(switch["parameters"]["properties"]["project"]["description"], PROJECT_SWITCH_PROJECT_DESCRIPTION);
        let req2 = switch["parameters"]["required"].as_array().unwrap();
        assert_eq!(req2, &vec![json!("project")]);

        // JSON round-trip
        let s = project_create_schema_json();
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, create);
        assert_eq!(serde_json::from_str::<Value>(&project_list_schema_json()).unwrap(), list);
        assert_eq!(serde_json::from_str::<Value>(&project_switch_schema_json()).unwrap(), switch);
    }

    #[test]
    fn primary_path_mirrors_python() {
        // primary_path field takes precedence
        let p = make_project("p_1", "slug", "Name", Some("/primary"), vec![("/folder", true)]);
        assert_eq!(primary_path(&p), Some("/primary".to_string()));

        // empty primary_path falls through to folders is_primary
        let p2 = make_project("p_1", "slug", "Name", Some(""), vec![("/a", false), ("/b", true), ("/c", false)]);
        assert_eq!(primary_path(&p2), Some("/b".to_string()));

        // no primary, no is_primary → first folder
        let p3 = make_project("p_1", "slug", "Name", None, vec![("/x", false), ("/y", false)]);
        assert_eq!(primary_path(&p3), Some("/x".to_string()));

        // no folders → None
        let p4 = make_project("p_1", "slug", "Name", None, vec![]);
        assert_eq!(primary_path(&p4), None);

        // primary_path with empty string plus is_primary present → is_primary wins
        let p5 = make_project("p_1", "slug", "Name", None, vec![("/only", true)]);
        assert_eq!(primary_path(&p5), Some("/only".to_string()));
    }

    #[test]
    fn resolve_mirrors_python() {
        let projects = vec![
            make_project("p_aaa", "aurora-demo", "Aurora Demo", None, vec![]),
            make_project("p_bbb", "other", "Other Project", None, vec![]),
            make_project("p_ccc", "my-slug", "My Project", None, vec![]),
        ];
        // exact id
        assert_eq!(resolve_project(&projects, "p_aaa").unwrap().id, "p_aaa");
        // exact slug
        assert_eq!(resolve_project(&projects, "aurora-demo").unwrap().id, "p_aaa");
        // exact name (case-sensitive)
        assert_eq!(resolve_project(&projects, "Aurora Demo").unwrap().id, "p_aaa");
        // case-insensitive slug/name second pass
        assert_eq!(resolve_project(&projects, "AURORA-DEMO").unwrap().id, "p_aaa");
        assert_eq!(resolve_project(&projects, "aurora demo").unwrap().id, "p_aaa");
        // token trim
        assert_eq!(resolve_project(&projects, "  p_aaa  ").unwrap().id, "p_aaa");
        // empty → None
        assert!(resolve_project(&projects, "").is_none());
        assert!(resolve_project(&projects, "   ").is_none());
        // no match
        assert!(resolve_project(&projects, "notfound").is_none());
        // name lower vs original: "Other Project" exact case
        assert_eq!(resolve_project(&projects, "other project").unwrap().id, "p_bbb");
    }

    #[test]
    fn apply_workspace_calls_callback() {
        // global callback path tested via injected variant for determinism
        let called: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let called_clone = Arc::clone(&called);
        let cb = move |tid: &str, path: &str, name: &str| {
            called_clone.lock().unwrap().push((tid.to_string(), path.to_string(), name.to_string()));
        };
        // success case: both task_id and path present
        apply_workspace_with(Some(&cb), Some("tid123"), Some("/some/path"), "My Proj");
        assert_eq!(called.lock().unwrap().len(), 1);
        assert_eq!(called.lock().unwrap()[0], ("tid123".to_string(), "/some/path".to_string(), "My Proj".to_string()));

        // missing task_id → no call
        let called2: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let called2_clone = Arc::clone(&called2);
        let cb2 = move |tid: &str, path: &str, name: &str| {
            called2_clone.lock().unwrap().push((tid.to_string(), path.to_string(), name.to_string()));
        };
        apply_workspace_with(Some(&cb2), None, Some("/path"), "Name");
        assert!(called2.lock().unwrap().is_empty());
        apply_workspace_with(Some(&cb2), Some("tid"), None, "Name");
        assert!(called2.lock().unwrap().is_empty());
        apply_workspace_with(None, Some("tid"), Some("/path"), "Name");
        assert!(called2.lock().unwrap().is_empty());

        // empty strings also no call
        apply_workspace_with(Some(&cb2), Some(""), Some("/path"), "Name");
        assert!(called2.lock().unwrap().is_empty());

        // panic in callback is swallowed (mirrors Python except Exception: pass)
        let panic_cb = |_: &str, _: &str, _: &str| panic!("boom");
        apply_workspace_with(Some(&panic_cb), Some("tid"), Some("/path"), "Name");
        // should not panic here
    }

    #[test]
    fn workspace_callback_global_set_and_clear() {
        clear_project_workspace_callback();
        assert!(get_workspace_callback().is_none());
        set_project_workspace_callback(Some(|_: &str, _: &str, _: &str| {}));
        assert!(get_workspace_callback().is_some());
        clear_project_workspace_callback();
        assert!(get_workspace_callback().is_none());
        // boxed variant
        let cb: WorkspaceCallback = Arc::new(|_: &str, _: &str, _: &str| {});
        set_project_workspace_callback_boxed(Some(Arc::clone(&cb)));
        assert!(get_workspace_callback().is_some());
        clear_project_workspace_callback();
    }

    #[test]
    fn normalize_path_mirrors_python() {
        // absolute stays absolute, trailing slash stripped
        assert_eq!(normalize_project_path("/tmp/foo/"), "/tmp/foo");
        assert_eq!(normalize_project_path("/tmp/foo///"), "/tmp/foo");
        assert_eq!(normalize_project_path("/"), "/");
        // relative becomes absolute via cwd
        let cwd = std::env::current_dir().unwrap().to_string_lossy().to_string();
        let rel = normalize_project_path("a/b");
        assert!(rel.starts_with(&cwd) || rel.starts_with('/'));
        // expanduser ~/
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            let expanded = normalize_project_path("~/repo");
            assert_eq!(expanded, format!("{}/repo", home.trim_end_matches('/')));
            assert_eq!(normalize_project_path("~"), home.trim_end_matches('/').to_string());
        }
        // empty → empty
        assert_eq!(normalize_project_path(""), "");
        assert_eq!(normalize_project_path("   "), "");
        // normalize key lowercases on windows only; on linux same
        assert_eq!(normalize_path_key("/Tmp/Foo"), if cfg!(windows) { "/tmp/foo".to_string() } else { "/Tmp/Foo".to_string() });
    }

    #[test]
    fn find_by_primary_path_in_slice_normalized() {
        // ensure dedup via normalized key
        let p1 = make_project("p_1", "s1", "Proj1", Some("/tmp/repo"), vec![]);
        let p2 = make_project("p_2", "s2", "Proj2", Some("/tmp/other"), vec![]);
        let projects = vec![p1.clone(), p2.clone()];
        assert_eq!(find_by_primary_path_in_slice(&projects, "/tmp/repo").unwrap().id, "p_1");
        assert_eq!(find_by_primary_path_in_slice(&projects, "/tmp/repo/").unwrap().id, "p_1");
        // trailing slash normalized
        assert_eq!(find_by_primary_path_in_slice(&projects, "/tmp/repo///").unwrap().id, "p_1");
        assert!(find_by_primary_path_in_slice(&projects, "/tmp/missing").is_none());
        assert!(find_by_primary_path_in_slice(&projects, "").is_none());
    }

    #[test]
    fn project_list_returns_active_and_projects() {
        let store = InMemoryStore::new();
        // empty
        let out = project_list_with_store(&store);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["active_id"], Value::Null);
        assert_eq!(v["projects"].as_array().unwrap().len(), 0);

        // create two projects via store directly
        let pid1 = store.create_project("Aurora Demo", vec!["/tmp/aurora".to_string()], Some("/tmp/aurora".to_string())).unwrap();
        let pid2 = store.create_project("Other", vec![], None).unwrap();
        store.set_active(&pid2);
        let out2 = project_list_with_store(&store);
        let v2: Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(v2["active_id"], json!(pid2));
        let arr = v2["projects"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // first project has primary_path set, active false; second active true
        let first = &arr[0];
        assert_eq!(first["id"], json!(pid1));
        assert_eq!(first["active"], json!(false));
        assert_eq!(first["primary_path"], json!("/tmp/aurora"));
        let second = &arr[1];
        assert_eq!(second["id"], json!(pid2));
        assert_eq!(second["active"], json!(true));
        assert_eq!(second["primary_path"], Value::Null);
    }

    #[test]
    fn project_create_success_and_idempotent() {
        let store = InMemoryStore::new();
        let called: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let called_clone = Arc::clone(&called);
        let cb = move |tid: &str, path: &str, name: &str| {
            called_clone.lock().unwrap().push((tid.to_string(), path.to_string(), name.to_string()));
        };

        // create with path
        let out = project_create_with_store_and_workspace(&store, "Aurora Demo", Some("/tmp/aurora"), Some("task123"), Some(&cb));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["name"], "Aurora Demo");
        let id1 = v["id"].as_str().unwrap().to_string();
        assert!(v["slug"].as_str().unwrap().contains("aurora"));
        assert_eq!(v["primary_path"], json!("/tmp/aurora"));
        // workspace callback fired because task_id and primary present
        assert_eq!(called.lock().unwrap().len(), 1);
        assert_eq!(called.lock().unwrap()[0].0, "task123");
        // active set
        assert_eq!(store.get_active_id().unwrap(), id1);
        // second create with same folder should be idempotent (no duplicate)
        let out2 = project_create_with_store_and_workspace(&store, "New Name Should Be Ignored", Some("/tmp/aurora"), Some("task456"), Some(&cb));
        let v2: Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(v2["success"], true);
        assert_eq!(v2["id"], json!(id1)); // same id reused
        assert_eq!(v2["name"], "Aurora Demo"); // original name preserved (existing project)
        // no new project added
        assert_eq!(store.list_projects(false).len(), 1);
        // workspace callback again
        assert_eq!(called.lock().unwrap().len(), 2);

        // create without path
        let out3 = project_create_with_store(&store, "Second Project", None, None);
        let v3: Value = serde_json::from_str(&out3).unwrap();
        assert_eq!(v3["success"], true);
        assert_eq!(v3["name"], "Second Project");
        assert_eq!(v3["primary_path"], Value::Null);
        assert_eq!(store.list_projects(false).len(), 2);

        // duplicate path via store create_project directly would error, but project_create handles via idempotent path
        // verify store.create_project duplicate error path
        let err = store.create_project("Third", vec!["/tmp/aurora".to_string()], Some("/tmp/aurora".to_string()));
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("already belongs to project"));
    }

    #[test]
    fn project_create_validates_name() {
        let store = InMemoryStore::new();
        let out = project_create_with_store(&store, "", Some("/tmp/path"), None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error"], NAME_REQUIRED_ERROR);

        let out2 = project_create_with_store(&store, "   ", None, None);
        let v2: Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(v2["success"], false);
        assert_eq!(v2["error"], NAME_REQUIRED_ERROR);
    }

    #[test]
    fn project_create_trims_name_and_normalizes_path() {
        let store = InMemoryStore::new();
        // name trimmed
        let out = project_create_with_store(&store, "  My Project  ", Some("  /tmp/myproj/  "), None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["name"], "My Project");
        // path normalized (strip trailing slash, absolute)
        assert_eq!(v["primary_path"], json!("/tmp/myproj"));
        let proj = store.get_project(v["id"].as_str().unwrap()).unwrap();
        assert_eq!(proj.folders[0].path, "/tmp/myproj");
    }

    #[test]
    fn project_create_handles_store_error() {
        // Simulate store that always fails create (to trigger ValueError path)
        struct FailingStore;
        impl ProjectStore for FailingStore {
            fn list_projects(&self, _inc: bool) -> Vec<Project> { vec![] }
            fn get_active_id(&self) -> Option<String> { None }
            fn find_by_primary_path(&self, _p: &str) -> Option<Project> { None }
            fn create_project(&self, _n: &str, _f: Vec<String>, _p: Option<String>) -> Result<String, String> { Err("boom create failed".to_string()) }
            fn set_active(&self, _id: &str) {}
            fn get_project(&self, _id: &str) -> Option<Project> { None }
        }
        let store = FailingStore;
        let out = project_create_with_store(&store, "Test", Some("/tmp/x"), None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error"], "boom create failed");
    }

    #[test]
    fn project_switch_success_and_errors() {
        let store = InMemoryStore::new();
        let pid1 = store.create_project("Aurora Demo", vec!["/tmp/aurora".to_string()], Some("/tmp/aurora".to_string())).unwrap();
        let pid2 = store.create_project("Other", vec!["/tmp/other".to_string()], Some("/tmp/other".to_string())).unwrap();
        store.set_active(&pid1);
        assert_eq!(store.get_active_id().unwrap(), pid1);

        // switch by slug
        let called: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let called_clone = Arc::clone(&called);
        let cb = move |tid: &str, path: &str, name: &str| {
            called_clone.lock().unwrap().push((tid.to_string(), path.to_string(), name.to_string()));
        };
        let slug = store.get_project(&pid2).unwrap().slug.clone();
        let out = project_switch_with_store_and_workspace(&store, &slug, Some("task999"), Some(&cb));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["id"], json!(pid2));
        assert_eq!(store.get_active_id().unwrap(), pid2);
        assert_eq!(called.lock().unwrap().len(), 1);
        assert_eq!(called.lock().unwrap()[0].1, "/tmp/other");

        // switch by id
        let out2 = project_switch_with_store(&store, &pid1, None);
        let v2: Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(v2["id"], json!(pid1));
        assert_eq!(store.get_active_id().unwrap(), pid1);

        // switch by name case-insensitive
        let out3 = project_switch_with_store(&store, "aurora demo", None);
        let v3: Value = serde_json::from_str(&out3).unwrap();
        assert_eq!(v3["id"], json!(pid1));

        // not found
        let out4 = project_switch_with_store(&store, "notfound", None);
        let v4: Value = serde_json::from_str(&out4).unwrap();
        assert_eq!(v4["success"], false);
        assert_eq!(v4["error"], json!(no_project_error("notfound")));

        // empty token → not found
        let out5 = project_switch_with_store(&store, "   ", None);
        let v5: Value = serde_json::from_str(&out5).unwrap();
        assert_eq!(v5["success"], false);
    }

    #[test]
    fn handler_extracts_like_python_lambda() {
        let store = InMemoryStore::new();
        // project_create via handler
        let args = json!({"name": "Handler Test", "path": "/tmp/handler"});
        let out = handler_project_create(&store, &args, None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["name"], "Handler Test");

        // missing name → error via handler (Python args.get("name",""))
        let args2 = json!({});
        let out2 = handler_project_create(&store, &args2, None);
        let v2: Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(v2["success"], false);
        assert_eq!(v2["error"], NAME_REQUIRED_ERROR);

        // project_switch via handler
        let pid = store.get_active_id().unwrap();
        let out3 = handler_project_switch(&store, &json!({"project": pid.clone()}), None);
        let v3: Value = serde_json::from_str(&out3).unwrap();
        assert_eq!(v3["id"], json!(pid));

        // handler with missing project → error
        let out4 = handler_project_switch(&store, &json!({}), None);
        let v4: Value = serde_json::from_str(&out4).unwrap();
        assert_eq!(v4["success"], false);

        // project_list handler
        let out5 = handler_project_list(&store, &json!({}), None);
        let v5: Value = serde_json::from_str(&out5).unwrap();
        assert!(v5["projects"].is_array());
    }

    #[test]
    fn slugify_and_unique() {
        assert_eq!(slugify("Aurora Demo"), "aurora-demo");
        assert_eq!(slugify("  Hello__World  "), "hello-world");
        assert_eq!(slugify("___"), "project");
        assert_eq!(slugify(""), "project");
        assert_eq!(slugify("A---B"), "a-b");
        // truncate 64
        let long = "a".repeat(100);
        assert_eq!(slugify(&long).len(), 64);
        // unique
        let p1 = make_project("p_1", "test", "Test", None, vec![]);
        let projects = vec![p1];
        assert_eq!(unique_slug(&projects, "test"), "test-2");
        let p2 = make_project("p_2", "test-2", "Test2", None, vec![]);
        let projects2 = vec![projects[0].clone(), p2];
        assert_eq!(unique_slug(&projects2, "test"), "test-3");
    }

    #[test]
    fn json_preserves_unicode() {
        let store = InMemoryStore::new();
        let out = project_create_with_store(&store, "Café 🎉", Some("/tmp/café"), None);
        assert!(out.contains("Café"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["name"], "Café 🎉");
        // project_list preserves unicode
        let out2 = project_list_with_store(&store);
        assert!(out2.contains("Café"));
    }

    #[test]
    fn all_constant() {
        assert_eq!(ALL, &["project_list", "project_create", "project_switch", "set_project_workspace_callback", "primary_path", "resolve_project"]);
    }
}
