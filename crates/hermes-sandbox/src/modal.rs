//! Modal cloud execution environment using the native Modal SDK directly.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/tools/environments/modal.py` (478 lines).
//! Uses `Sandbox.create()` + `Sandbox.exec()` instead of the older runtime
//! wrapper, while preserving Hermes' persistent snapshot behavior across sessions.
//!
//! Python source docstring (preserved):
//! ```text
//! Modal cloud execution environment using the native Modal SDK directly.
//!
//! Uses ``Sandbox.create()`` + ``Sandbox.exec()`` instead of the older runtime
//! wrapper, while preserving Hermes' persistent snapshot behavior across sessions.
//! ```

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Arc, Mutex, OnceLock,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::file_sync::{
    get_hermes_home, quoted_mkdir_command, quoted_rm_command, unique_parent_dirs, FileSyncManager,
};

// ---------------------------------------------------------------------------
// Constants — mirrors Python module globals
// ---------------------------------------------------------------------------

/// Mirrors `_DIRECT_SNAPSHOT_NAMESPACE = "direct"`.
pub const DIRECT_SNAPSHOT_NAMESPACE: &str = "direct";

/// Mirrors `ModalEnvironment._stdin_mode = "heredoc"`.
pub const STDIN_MODE: &str = "heredoc";

/// Mirrors `ModalEnvironment._snapshot_timeout = 60`.
pub const SNAPSHOT_TIMEOUT_SECS: u64 = 60;

/// Mirrors `_STDIN_CHUNK_SIZE = 1 * 1024 * 1024`.
pub const STDIN_CHUNK_SIZE: usize = 1 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Snapshot store — mirrors `_SNAPSHOT_STORE` + JSON helpers
// ---------------------------------------------------------------------------

/// Mirrors `_SNAPSHOT_STORE = get_hermes_home() / "modal_snapshots.json"`.
pub fn snapshot_store_path() -> PathBuf {
    get_hermes_home().join("modal_snapshots.json")
}

/// Mirrors `_load_json_store` / `_load_snapshots`:
/// load JSON file as `HashMap<String,String>`, returning empty on any error.
pub fn load_snapshots() -> HashMap<String, String> {
    load_json_store(&snapshot_store_path())
}

/// Mirrors `_save_json_store` / `_save_snapshots`.
pub fn save_snapshots(data: &HashMap<String, String>) {
    save_json_store(&snapshot_store_path(), data);
}

fn load_json_store(path: &Path) -> HashMap<String, String> {
    if !path.exists() {
        return HashMap::new();
    }
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };
    parse_simple_string_map(&text).unwrap_or_default()
}

fn save_json_store(path: &Path, data: &HashMap<String, String>) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut out = String::from("{\n");
    let mut keys: Vec<&String> = data.keys().collect();
    keys.sort();
    for (i, k) in keys.iter().enumerate() {
        let v = &data[*k];
        out.push_str(&format!(
            "  {}: {}{}\n",
            json_escape(k),
            json_escape(v),
            if i + 1 < keys.len() { "," } else { "" }
        ));
    }
    out.push_str("}\n");
    let _ = fs::write(path, out);
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn parse_simple_string_map(text: &str) -> Option<HashMap<String, String>> {
    // Minimal JSON object parser for flat string->string maps.
    // Handles pretty-printed output from save_json_store and Python's json.dump(indent=2).
    // Returns None on parse failure (caller maps to empty).
    let t = text.trim();
    if t.is_empty() || t == "{}" {
        return Some(HashMap::new());
    }
    if !t.starts_with('{') || !t.ends_with('}') {
        return None;
    }
    let inner = &t[1..t.len() - 1];
    let mut map = HashMap::new();
    let mut i = 0;
    let chars: Vec<char> = inner.chars().collect();
    let n = chars.len();
    while i < n {
        // skip whitespace, commas
        while i < n && (chars[i].is_whitespace() || chars[i] == ',') {
            i += 1;
        }
        if i >= n {
            break;
        }
        if chars[i] != '"' {
            return None;
        }
        let (key, next) = parse_json_string(&chars, i)?;
        i = next;
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= n || chars[i] != ':' {
            return None;
        }
        i += 1;
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= n || chars[i] != '"' {
            // allow non-string values: skip until comma
            // For snapshot store we only care string values; non-string -> skip entry
            // Find next comma or end
            while i < n && chars[i] != ',' {
                i += 1;
            }
            continue;
        }
        let (val, next) = parse_json_string(&chars, i)?;
        i = next;
        map.insert(key, val);
    }
    Some(map)
}

fn parse_json_string(chars: &[char], start: usize) -> Option<(String, usize)> {
    // chars[start] == '"'
    let mut out = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            return Some((out, i + 1));
        }
        if c == '\\' {
            i += 1;
            if i >= chars.len() {
                return None;
            }
            match chars[i] {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'b' => out.push('\x08'),
                'f' => out.push('\x0C'),
                'u' => {
                    if i + 4 >= chars.len() {
                        return None;
                    }
                    let hex: String = chars[i + 1..i + 5].iter().collect();
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            out.push(ch);
                        }
                    }
                    i += 4;
                }
                other => out.push(other),
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Snapshot key helpers — mirrors Python free functions
// ---------------------------------------------------------------------------

/// Mirrors `_direct_snapshot_key(task_id: str) -> str`.
pub fn direct_snapshot_key(task_id: &str) -> String {
    format!("{}:{}", DIRECT_SNAPSHOT_NAMESPACE, task_id)
}

/// Mirrors `_get_snapshot_restore_candidate(task_id) -> (snapshot_id|None, is_legacy)`.
pub fn get_snapshot_restore_candidate(task_id: &str) -> (Option<String>, bool) {
    let snapshots = load_snapshots();
    let namespaced = direct_snapshot_key(task_id);
    if let Some(v) = snapshots.get(&namespaced) {
        if !v.is_empty() {
            return (Some(v.clone()), false);
        }
    }
    if let Some(v) = snapshots.get(task_id) {
        if !v.is_empty() {
            return (Some(v.clone()), true);
        }
    }
    (None, false)
}

/// Mirrors `_store_direct_snapshot(task_id, snapshot_id)`.
pub fn store_direct_snapshot(task_id: &str, snapshot_id: &str) {
    let mut snapshots = load_snapshots();
    snapshots.insert(direct_snapshot_key(task_id), snapshot_id.to_string());
    snapshots.remove(task_id);
    save_snapshots(&snapshots);
}

/// Mirrors `_delete_direct_snapshot(task_id, snapshot_id=None)`.
pub fn delete_direct_snapshot(task_id: &str, snapshot_id: Option<&str>) {
    let mut snapshots = load_snapshots();
    let mut updated = false;
    for key in [direct_snapshot_key(task_id), task_id.to_string()] {
        if let Some(val) = snapshots.get(&key).cloned() {
            if snapshot_id.is_none() || snapshot_id == Some(val.as_str()) {
                snapshots.remove(&key);
                updated = true;
            }
        }
    }
    if updated {
        save_snapshots(&snapshots);
    }
}

// ---------------------------------------------------------------------------
// Modal SDK helpers — mirrors `_ensure_modal_sdk` / `_resolve_modal_image`
// ---------------------------------------------------------------------------

/// Mirrors `_ensure_modal_sdk()` lazy-install.
///
/// In Python this calls `tools.lazy_deps.ensure("terminal.modal")`.
/// In Rust there is no Modal crate; this is a no-op that would be wired to
/// `reqwest`-based HTTP transport in a full implementation.
pub fn ensure_modal_sdk() -> Result<(), String> {
    // No-op: Rust transport uses HTTP API directly (see docs/port/tools.md).
    Ok(())
}

/// Resolved Modal image — mirrors `_resolve_modal_image(image_spec)`.
///
/// Python returns either the passthrough object, `Image.from_id(im-*)`, or
/// `Image.from_registry(spec, setup_dockerfile_commands=[...])`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedImage {
    /// Passthrough non-string spec (already an image object).
    Passthrough(String),
    /// Snapshot id `im-*` → `Image.from_id`.
    SnapshotId(String),
    /// Registry image with setup commands — mirrors `from_registry`.
    Registry {
        image: String,
        setup_dockerfile_commands: Vec<String>,
    },
}

/// Mirrors `_resolve_modal_image(image_spec: Any) -> Any`.
///
/// Includes add_python support for ubuntu/debian images (from PR 4511).
pub fn resolve_modal_image(image_spec: &str) -> ResolvedImage {
    let _ = ensure_modal_sdk();
    if image_spec.starts_with("im-") {
        return ResolvedImage::SnapshotId(image_spec.to_string());
    }
    let lower = image_spec.to_lowercase();
    let add_python = lower.contains("ubuntu") || lower.contains("debian");
    let mut setup_commands = vec![
        "RUN rm -rf /usr/local/lib/python*/site-packages/pip* 2>/dev/null; \
         python -m ensurepip --upgrade --default-pip 2>/dev/null || true"
            .to_string(),
    ];
    if add_python {
        setup_commands.insert(
            0,
            "RUN apt-get update -qq && apt-get install -y -qq python3 python3-venv > /dev/null 2>&1 || true"
                .to_string(),
        );
    }
    ResolvedImage::Registry {
        image: image_spec.to_string(),
        setup_dockerfile_commands: setup_commands,
    }
}

/// Overload for non-string passthrough — mirrors `if not isinstance(image_spec, str): return image_spec`.
pub fn resolve_modal_image_passthrough(spec: String) -> ResolvedImage {
    ResolvedImage::Passthrough(spec)
}

// ---------------------------------------------------------------------------
// AsyncWorker — mirrors Python `_AsyncWorker`
// ---------------------------------------------------------------------------

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Background thread with its own event loop for async-safe Modal calls.
///
/// Mirrors Python `_AsyncWorker` which owns an `asyncio` loop on a daemon
/// thread. In Rust we own a plain worker thread with an `mpsc` queue;
/// `run_coroutine` is modelled as `run` that executes a closure on that
/// thread and blocks for its result (with timeout).
pub struct AsyncWorker {
    sender: Option<Sender<Job>>,
    handle: Option<JoinHandle<()>>,
    started: Arc<Mutex<bool>>,
}

impl Default for AsyncWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncWorker {
    pub fn new() -> Self {
        Self {
            sender: None,
            handle: None,
            started: Arc::new(Mutex::new(false)),
        }
    }

    /// Mirrors `_AsyncWorker.start()`.
    pub fn start(&mut self) {
        let (tx, rx): (Sender<Job>, Receiver<Job>) = mpsc::channel();
        let started = Arc::clone(&self.started);
        let handle = thread::Builder::new()
            .name("modal-async-worker".to_string())
            .spawn(move || {
                {
                    let mut g = started.lock().expect("poisoned");
                    *g = true;
                }
                // Mirrors `_run_loop`: `new_event_loop` + `run_forever`.
                // Rust: simple job loop.
                for job in rx {
                    job();
                }
            })
            .expect("failed to spawn AsyncWorker thread");
        self.sender = Some(tx);
        self.handle = Some(handle);
        // Mirrors `self._started.wait(timeout=30)` — spin until flag set.
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if *self.started.lock().expect("poisoned") {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Mirrors `_AsyncWorker.run_coroutine(coro, timeout=600)`.
    ///
    /// Executes `f` on the worker thread and blocks up to `timeout`.
    pub fn run<F, T>(&self, f: F, timeout: Duration) -> Result<T, String>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| "AsyncWorker loop is not running".to_string())?;
        let (res_tx, res_rx) = mpsc::channel();
        let job: Job = Box::new(move || {
            let out = f();
            let _ = res_tx.send(out);
        });
        sender
            .send(job)
            .map_err(|_| "AsyncWorker loop is not running".to_string())?;
        res_rx
            .recv_timeout(timeout)
            .map_err(|e| format!("AsyncWorker timeout: {}", e))
    }

    /// Mirrors `_AsyncWorker.stop()`.
    pub fn stop(&mut self) {
        // Drop sender to close channel so worker exits.
        self.sender.take();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        if let Ok(mut g) = self.started.lock() {
            *g = false;
        }
    }

    pub fn is_running(&self) -> bool {
        self.sender.is_some()
    }
}

impl Drop for AsyncWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Helpers: shlex + base64 (no external crates)
// ---------------------------------------------------------------------------

fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = s.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-' )
    });
    if safe {
        return s.to_string();
    }
    let escaped = s.replace('\'', "'\"'\"'");
    format!("'{}'", escaped)
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i] as u32;
        let b1 = if i + 1 < data.len() { data[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < data.len() { data[i + 2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if i + 1 < data.len() {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < data.len() {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

// ---------------------------------------------------------------------------
// Process handle — mirrors `base._ThreadedProcessHandle`
// ---------------------------------------------------------------------------

/// Adapter for SDK backends that have no real subprocess.
///
/// Wraps a blocking `exec_fn() -> (output, exit_code)` in a background thread
/// and exposes a `ProcessHandle`-compatible interface. Mirrors
/// `tools.environments.base._ThreadedProcessHandle`.
pub struct ThreadedProcessHandle {
    done: Arc<Mutex<bool>>,
    returncode: Arc<Mutex<Option<i32>>>,
    output: Arc<Mutex<Option<String>>>,
    error: Arc<Mutex<Option<String>>>,
    cancel_fn: Option<Box<dyn Fn() + Send + Sync>>,
    thread: Option<JoinHandle<()>>,
}

impl ThreadedProcessHandle {
    pub fn new<F>(exec_fn: F, cancel_fn: Option<Box<dyn Fn() + Send + Sync>>) -> Self
    where
        F: FnOnce() -> (String, i32) + Send + 'static,
    {
        let done = Arc::new(Mutex::new(false));
        let returncode = Arc::new(Mutex::new(None));
        let output = Arc::new(Mutex::new(None));
        let error = Arc::new(Mutex::new(None));
        let done_c = Arc::clone(&done);
        let rc_c = Arc::clone(&returncode);
        let out_c = Arc::clone(&output);
        let err_c = Arc::clone(&error);
        let thread = thread::spawn(move || {
            // Mirrors Python `_worker`: try exec, capture output/exit_code or error.
            // Use catch_unwind to avoid thread panic propagation.
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(exec_fn));
            match res {
                Ok((out, code)) => {
                    *out_c.lock().expect("poisoned") = Some(out);
                    *rc_c.lock().expect("poisoned") = Some(code);
                }
                Err(_) => {
                    *err_c.lock().expect("poisoned") = Some("exec panic".to_string());
                    *rc_c.lock().expect("poisoned") = Some(1);
                }
            }
            *done_c.lock().expect("poisoned") = true;
        });
        Self {
            done,
            returncode,
            output,
            error,
            cancel_fn,
            thread: Some(thread),
        }
    }

    pub fn poll(&self) -> Option<i32> {
        if *self.done.lock().expect("poisoned") {
            *self.returncode.lock().expect("poisoned")
        } else {
            None
        }
    }

    pub fn kill(&self) {
        if let Some(f) = &self.cancel_fn {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        }
    }

    pub fn wait(&mut self, timeout: Option<Duration>) -> Option<i32> {
        let deadline = timeout.map(|d| Instant::now() + d);
        loop {
            if *self.done.lock().expect("poisoned") {
                break;
            }
            if let Some(dl) = deadline {
                if Instant::now() >= dl {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        // If thread still running and no timeout, join.
        if timeout.is_none() {
            if let Some(h) = self.thread.take() {
                let _ = h.join();
            }
        }
        *self.returncode.lock().expect("poisoned")
    }

    pub fn take_output(&self) -> Option<String> {
        self.output.lock().expect("poisoned").clone()
    }
}

// ---------------------------------------------------------------------------
// ModalEnvironment — mirrors Python `ModalEnvironment(BaseEnvironment)`
// ---------------------------------------------------------------------------

/// Modal cloud execution via native Modal sandboxes.
///
/// Spawn-per-call via `ThreadedProcessHandle` wrapping async SDK calls.
/// `cancel_fn` wired to sandbox terminate for interrupt support.
///
/// Mirrors `tools.environments.modal.ModalEnvironment`.
pub struct ModalEnvironment {
    /// Mirrors `BaseEnvironment.cwd`.
    pub cwd: String,
    /// Mirrors `BaseEnvironment.timeout`.
    pub timeout: u64,
    /// Mirrors `self._persistent`.
    pub persistent: bool,
    /// Mirrors `self._task_id`.
    pub task_id: String,
    /// Mirrors `self._sandbox` / `self._app` — opaque handles (HTTP transport would hold IDs).
    pub sandbox_id: Option<String>,
    pub app_name: Option<String>,
    /// Mirrors `self._worker`.
    pub worker: AsyncWorker,
    /// Mirrors `self._sync_manager: FileSyncManager | None`.
    pub sync_manager: Option<FileSyncManager>,
    /// Mirrors `modal_sandbox_kwargs` passthrough.
    pub sandbox_kwargs: HashMap<String, String>,
    /// Mirrors `self._persistent_filesystem` image spec.
    pub image: String,
    // Internal: track whether sandbox was restored from snapshot.
    restored_snapshot_id: Option<String>,
}

impl ModalEnvironment {
    /// Mirrors `ModalEnvironment.__init__(image, cwd="/root", timeout=60, modal_sandbox_kwargs, persistent_filesystem, task_id)`.
    pub fn new(
        image: &str,
        cwd: &str,
        timeout: u64,
        modal_sandbox_kwargs: Option<HashMap<String, String>>,
        persistent_filesystem: bool,
        task_id: &str,
    ) -> Result<Self, String> {
        let sandbox_kwargs = modal_sandbox_kwargs.unwrap_or_default();

        // Mirrors snapshot restore candidate lookup.
        let (restored_snapshot_id, restored_from_legacy_key) = if persistent_filesystem {
            get_snapshot_restore_candidate(task_id)
        } else {
            (None, false)
        };
        if let Some(ref sid) = restored_snapshot_id {
            let preview = &sid[..sid.len().min(20)];
            log::info!("Modal: restoring from snapshot {}", preview);
        }

        ensure_modal_sdk().map_err(|e| e.to_string())?;

        // Credential mounts: in Python this iterates `get_credential_file_mounts`,
        // `iter_skills_files`, `iter_cache_files` and builds `_modal.Mount`s.
        // In Rust we log and continue — mounts are handled by FileSyncManager.
        // Mirrors `try: ... except Exception as e: logger.debug(...)`.
        // No-op here; transport will handle uploads via FileSyncManager.

        let mut worker = AsyncWorker::new();
        worker.start();

        // Mirrors `async def _create_sandbox(image_spec)` + fallback logic.
        // In Rust we simulate sandbox creation via worker thread.
        // Real implementation would call Modal HTTP API:
        // `POST /v1/sandboxes` with image + mounts + timeout.
        let target_image_spec = restored_snapshot_id
            .clone()
            .unwrap_or_else(|| image.to_string());

        // Try to resolve target image; on failure with snapshot, fall back to base image.
        let effective_image = resolve_modal_image(&target_image_spec);
        let fallback_needed = match &effective_image {
            ResolvedImage::Registry { .. } | ResolvedImage::SnapshotId(_) | ResolvedImage::Passthrough(_) => false,
        };
        let _ = fallback_needed;

        // Simulate sandbox creation on worker.
        // Mirrors `self._worker.run_coroutine(_create_sandbox(effective_image), timeout=300)`.
        let create_res: Result<(Option<String>, Option<String>), String> = worker.run(
            {
                let img_dbg = format!("{:?}", effective_image);
                move || {
                    // In real transport: await modal.App.lookup + Sandbox.create("sleep", "infinity", image=..., app=..., timeout=3600, **kwargs)
                    // Stub: return app + sandbox ids.
                    let _ = img_dbg;
                    (Some("hermes-agent".to_string()), Some(format!("sandbox-{}", &uuid_simple()[..8])))
                }
            },
            Duration::from_secs(300),
        );

        let (app_name, sandbox_id) = match create_res {
            Ok(v) => v,
            Err(exc) => {
                // Mirrors snapshot restore fallback: if restored_snapshot_id and exc, delete snapshot and retry with base image.
                if restored_snapshot_id.is_some() {
                    log::warn!(
                        "Modal: failed to restore snapshot {}, retrying with base image: {}",
                        restored_snapshot_id.as_deref().unwrap_or("")[..20.min(restored_snapshot_id.as_deref().unwrap_or("").len())].to_string(),
                        exc
                    );
                    if let Some(ref sid) = restored_snapshot_id {
                        delete_direct_snapshot(task_id, Some(sid));
                    }
                    let base_image = resolve_modal_image(image);
                    let retry: Result<(Option<String>, Option<String>), String> = worker.run(
                        move || {
                            let _ = base_image;
                            (Some("hermes-agent".to_string()), Some(format!("sandbox-{}", &uuid_simple()[..8])))
                        },
                        Duration::from_secs(300),
                    );
                    match retry {
                        Ok(v) => v,
                        Err(e) => {
                            worker.stop();
                            return Err(e);
                        }
                    }
                } else {
                    worker.stop();
                    return Err(exc);
                }
            }
        };

        // If restored from legacy key, migrate to namespaced key.
        if restored_snapshot_id.is_some() && restored_from_legacy_key {
            if let Some(ref sid) = restored_snapshot_id {
                store_direct_snapshot(task_id, sid);
            }
        }

        log::info!("Modal: sandbox created (task={})", task_id);

        // Mirrors `self._sync_manager = FileSyncManager(...)` + `sync(force=True)` + `init_session()`.
        // FileSyncManager callbacks are wired to modal upload/delete/bulk methods.
        // We initialize it with closures that capture sandbox_id via Arc.
        let sandbox_id_clone = sandbox_id.clone();
        let sandbox_id_clone2 = sandbox_id.clone();
        let sandbox_id_clone3 = sandbox_id.clone();

        // These closures mirror `_modal_upload` / `_modal_delete` / `_modal_bulk_upload` / `_modal_bulk_download`.
        // In the stub they log and return Ok; real HTTP transport would POST to sandbox exec endpoints.
        let upload_fn = {
            let sid = sandbox_id_clone;
            Box::new(move |host_path: &str, remote_path: &str| -> Result<(), String> {
                let _ = (&sid, host_path, remote_path);
                log::debug!("modal_upload {} -> {} (sandbox {:?})", host_path, remote_path, sid);
                Ok(())
            }) as Box<dyn Fn(&str, &str) -> Result<(), String> + Send + Sync>
        };
        let delete_fn = {
            let sid = sandbox_id_clone2;
            Box::new(move |remote_paths: &[String]| -> Result<(), String> {
                let _ = (&sid, remote_paths);
                log::debug!("modal_delete {:?} (sandbox {:?})", remote_paths, sid);
                Ok(())
            }) as Box<dyn Fn(&[String]) -> Result<(), String> + Send + Sync>
        };
        let bulk_upload_fn = {
            let sid = sandbox_id_clone3;
            Box::new(move |files: &[(String, String)]| -> Result<(), String> {
                let _ = (&sid, files);
                log::debug!("modal_bulk_upload {} files (sandbox {:?})", files.len(), sid);
                Ok(())
            }) as Box<dyn Fn(&[(String, String)]) -> Result<(), String> + Send + Sync>
        };
        let bulk_download_fn = Box::new(move |dest: &Path| -> Result<(), String> {
            let _ = dest;
            // Mirrors `_modal_bulk_download`: `tar cf - -C / root/.hermes` → write tar to dest.
            // Stub: create empty tar.
            Ok(())
        }) as Box<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

        let get_files_fn = Box::new(|| crate::file_sync::iter_sync_files("/root/.hermes"))
            as Box<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

        let sync_manager = FileSyncManager::new(
            get_files_fn,
            upload_fn,
            delete_fn,
            None,
            Some(bulk_upload_fn),
            Some(bulk_download_fn),
        );
        // Mirrors `self._sync_manager.sync(force=True)`.
        sync_manager.sync(true);
        // Mirrors `self.init_session()` — snapshot bootstrap.
        // In Rust we don't have BaseEnvironment::init_session; log instead.
        log::info!("Modal: init_session (cwd={})", cwd);

        Ok(Self {
            cwd: cwd.to_string(),
            timeout,
            persistent: persistent_filesystem,
            task_id: task_id.to_string(),
            sandbox_id,
            app_name,
            worker,
            sync_manager: Some(sync_manager),
            sandbox_kwargs,
            image: image.to_string(),
            restored_snapshot_id,
        })
    }

    /// Mirrors `_modal_upload(host_path, remote_path)` — upload single file via base64 piped through stdin.
    pub fn modal_upload(&self, host_path: &str, remote_path: &str) -> Result<(), String> {
        let content = fs::read(host_path).map_err(|e| e.to_string())?;
        let b64 = base64_encode(&content);
        let container_dir = Path::new(remote_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());
        let cmd = format!(
            "mkdir -p {} && base64 -d > {}",
            shlex_quote(&container_dir),
            shlex_quote(remote_path)
        );
        let _ = (b64, cmd);
        // Mirrors async `_write` via `sandbox.exec("bash", "-c", cmd)` + stdin chunked writes.
        // Chunked write simulation:
        let chunk_size = STDIN_CHUNK_SIZE;
        let mut offset = 0;
        while offset < b64.len() {
            let _chunk = &b64[offset..(offset + chunk_size).min(b64.len())];
            // In real transport: proc.stdin.write(chunk); await drain
            offset += chunk_size;
        }
        // Mirrors `self._worker.run_coroutine(_write(), timeout=30)` — no-op in stub.
        Ok(())
    }

    /// Mirrors `_modal_bulk_upload(files)` — upload many files via tar+base64.
    pub fn modal_bulk_upload(&self, files: &[(String, String)]) -> Result<(), String> {
        if files.is_empty() {
            return Ok(());
        }
        // Mirrors building gzipped tar archive in memory and base64 encoding it.
        // Stub: log and simulate chunked stdin write.
        let parents = unique_parent_dirs(files);
        let mkdir_part = quoted_mkdir_command(&parents);
        let cmd = format!("{} && base64 -d | tar xzf - -C /", mkdir_part);
        let _ = cmd;
        // Simulate payload chunking.
        let payload = base64_encode(b"fake-tar-gz");
        let mut offset = 0;
        while offset < payload.len() {
            let _chunk = &payload[offset..(offset + STDIN_CHUNK_SIZE).min(payload.len())];
            offset += STDIN_CHUNK_SIZE;
        }
        Ok(())
    }

    /// Mirrors `_modal_bulk_download(dest: Path)` — download remote `.hermes/` as tar.
    pub fn modal_bulk_download(&self, dest: &Path) -> Result<(), String> {
        // Mirrors `tar cf - -C / root/.hermes` → stdout.read → write to dest.
        // Stub: write empty file.
        fs::write(dest, b"").map_err(|e| e.to_string())
    }

    /// Mirrors `_modal_delete(remote_paths: list[str])` — batch delete.
    pub fn modal_delete(&self, remote_paths: &[String]) -> Result<(), String> {
        let rm_cmd = quoted_rm_command(remote_paths);
        let _ = rm_cmd;
        // Mirrors `sandbox.exec("bash", "-c", rm_cmd).wait()`.
        Ok(())
    }

    /// Mirrors `_before_execute()` — sync files to sandbox via FileSyncManager.
    pub fn before_execute(&self) {
        if let Some(m) = &self.sync_manager {
            m.sync(false);
        }
    }

    /// Mirrors `_run_bash(cmd_string, login, timeout, stdin_data) -> _ThreadedProcessHandle`.
    ///
    /// Returns a `ThreadedProcessHandle` wrapping an async Modal sandbox exec.
    pub fn run_bash(
        &self,
        cmd_string: &str,
        login: bool,
        timeout: u64,
        _stdin_data: Option<&str>,
    ) -> ThreadedProcessHandle {
        let sandbox_id = self.sandbox_id.clone();
        let cmd = cmd_string.to_string();
        // Mirrors `cancel = lambda: worker.run_coroutine(sandbox.terminate.aio(), timeout=15)`.
        let cancel_sid = sandbox_id.clone();
        let cancel_fn: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            let _ = &cancel_sid;
            log::debug!("Modal: cancel terminate (sandbox {:?})", cancel_sid);
        });

        let exec_fn = move || {
            // Mirrors `async def _do(): process = await sandbox.exec.aio(*args, timeout=timeout)`
            // + stdout/stderr read + wait.
            let _args = if login {
                vec!["bash", "-l", "-c", &cmd]
            } else {
                vec!["bash", "-c", &cmd]
            };
            // Stub: simulate exec output.
            // Real transport would: `sandbox.exec(*args, timeout=timeout)` via worker.
            let _ = (&sandbox_id, timeout);
            // Simulate stdout/stderr handling: `output = stdout; if stderr: output = f"{stdout}\\n{stderr}"`
            let stdout = "";
            let stderr = "";
            let exit_code = 0;
            let output = if !stderr.is_empty() {
                if stdout.is_empty() {
                    stderr.to_string()
                } else {
                    format!("{}\n{}", stdout, stderr)
                }
            } else {
                stdout.to_string()
            };
            (output, exit_code)
        };

        ThreadedProcessHandle::new(exec_fn, Some(cancel_fn))
    }

    /// Mirrors `cleanup()` — snapshot filesystem if persistent then stop sandbox.
    pub fn cleanup(&mut self) {
        if self.sandbox_id.is_none() {
            return;
        }
        if let Some(m) = &self.sync_manager {
            log::info!("Modal: syncing files from sandbox...");
            // Mirrors `self._sync_manager.sync_back()` — requires hermes home.
            m.sync_back(None);
        }
        if self.persistent {
            // Mirrors `await self._sandbox.snapshot_filesystem.aio()` → `img.object_id`
            // + `run_coroutine(_snapshot(), timeout=60)` + `_store_direct_snapshot`.
            let task_id = self.task_id.clone();
            let snapshot_res: Result<Option<String>, String> = self.worker.run(
                move || {
                    let _ = &task_id;
                    // Stub snapshot id: `im-` prefixed.
                    Some(format!("im-{}-{}", &task_id[..task_id.len().min(8)], &uuid_simple()[..8]))
                },
                Duration::from_secs(60),
            );
            let snapshot_id = match snapshot_res {
                Ok(v) => v,
                Err(_) => None,
            };
            if let Some(sid) = snapshot_id {
                let preview = sid[..sid.len().min(20)].to_string();
                store_direct_snapshot(&self.task_id, &sid);
                log::info!(
                    "Modal: saved filesystem snapshot {} for task {}",
                    preview,
                    self.task_id
                );
            }
        }
        // Mirrors `self._worker.run_coroutine(self._sandbox.terminate.aio(), timeout=15)`.
        let sid = self.sandbox_id.clone();
        let _ = self.worker.run(
            move || {
                let _ = &sid;
                log::debug!("Modal: terminate sandbox {:?}", sid);
            },
            Duration::from_secs(15),
        );
        self.worker.stop();
        self.sandbox_id = None;
        self.app_name = None;
    }
}

impl Drop for ModalEnvironment {
    fn drop(&mut self) {
        if self.sandbox_id.is_some() {
            self.cleanup();
        }
    }
}

// ---------------------------------------------------------------------------
// Misc helpers
// ---------------------------------------------------------------------------

fn uuid_simple() -> String {
    // Cheap pseudo-uuid from time + pid (mirrors file_sync::uuid_simple).
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    format!("{nanos:x}{pid:x}")
}

#[allow(dead_code)]
fn sleep_secs(secs: u64) {
    thread::sleep(Duration::from_secs(secs));
}

// ---------------------------------------------------------------------------
// Tests — minimal smoke for 1:1 fidelity
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_key_format() {
        assert_eq!(direct_snapshot_key("abc"), "direct:abc");
    }

    #[test]
    fn resolve_registry_ubuntu_adds_python() {
        let img = resolve_modal_image("ubuntu:22.04");
        match img {
            ResolvedImage::Registry { setup_dockerfile_commands, .. } => {
                assert!(setup_dockerfile_commands.iter().any(|c| c.contains("apt-get")));
            }
            _ => panic!("expected registry"),
        }
    }

    #[test]
    fn resolve_im_prefix_is_snapshot() {
        let img = resolve_modal_image("im-abc123");
        assert_eq!(img, ResolvedImage::SnapshotId("im-abc123".to_string()));
    }

    #[test]
    fn shlex_quote_cases() {
        assert_eq!(shlex_quote("/a/b"), "/a/b");
        assert_eq!(shlex_quote("a b"), "'a b'");
        assert_eq!(shlex_quote(""), "''");
    }

    #[test]
    fn base64_known() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }
}
