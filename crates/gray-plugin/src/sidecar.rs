//! Sidecar plugin protocol (v1). See `docs/protocol-v1.md` for the
//! versioned spec (methods, TTLs, gating, host-emission audit).
//!
//! Transport: newline-delimited JSON over child stdio. Requests the host
//! sends are `{"id", "method", "params?"}`; sidecars reply with
//! `{"id", "result"}` for request/response methods only:
//! - `plugin/manifest` (request): no params, reply
//!   `{"name","version","tools":[{"name","description","parameters"}],
//!   "commands":["/x"],"hooks":[...]}`. Pre-v1 `"tools":["name"]` still parses.
//! - `tool/call` (request): params `{"name","args"}`, reply `{"content","is_error?"}`.
//! - `prompt/context` (request): params `{"cwd"}`, reply `{"text"}`.
//! - `tool/before` (request): params `{"name","args"}`, reply allow/deny/modify.
//! - `command/run` (request): params `{"name":"/x","argv"}`, reply `{"text"}`.
//! - `event/notify` (notification): NO `id`, NO reply expected. Params carry a
//!   minimal tagged event `{"type", ...}` where type is one of
//!   `pre_tool` | `post_tool` | `turn_end` with only the fields
//!   the sidecar needs (tool name/args, output content, usage totals).
//!
//! Sidecar→host requests (string `id`, `method: "host/..."`) are served by
//! the host handler: `host/run` (sub-agent turn), `host/say` (chat line),
//! `host/ask` (blocking user question; see `HOST_ASK`). Each one is
//! capability-gated (see [`crate::capabilities`]): the plugin declares the
//! surfaces it wants, and the host grants or refuses them. An ungranted
//! call gets a structured error, never the host's power. `host/ask` has an
//! extended outer deadline (see `ASK_TTL`): everything else keeps the 30s
//! TTL since only a human answer can legitimately take minutes.
//!
//! Unknown methods/lines are ignored.
//!
//! The three v1 request methods are only sent to sidecars claiming them in
//! `hooks`/`commands`, so pre-v1 sidecars (which ignore unknown lines, hence
//! would never reply) keep working.
//!
//! Concurrency: one reader task per sidecar routes replies by `id` into
//! `pending`; writers take a short stdin lock only, so concurrent requests
//! resolve out of order instead of serializing on one mutex.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};
use tokio::time::{Duration, timeout};

use gray_core::agent::{CommandOutcome, Tool, ToolContext, ToolOutput};
use gray_core::credential::CredentialMaterial;
use gray_core::message::ToolDef;

use crate::{
    CoreEvent, Manifest, PROVIDER_CREDENTIALS, Plugin, ProviderAuthPoll, ProviderAuthStart,
    ProviderModelCatalog, ProviderModelsRequest, ProviderRefreshRequest, ProviderRevokeRequest,
    ProviderRevokeResult, ProviderRpcError, ToolBefore, manifest_tools,
};

/// Plugin→host request handler (`host/run`, `host/say`). Set by the host via
/// [`SidecarPlugin::set_host_handler`]; without one the transport replies
/// `{"error": ...}` so plugin-initiated turns fail loudly, never hang.
/// Boxed-future shape (not `async_trait`) so the reader task can hold it
/// behind a plain `Mutex` without an extra dependency.
pub type HostHandler = Arc<
    dyn Fn(
            String,
            serde_json::Value,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = serde_json::Value> + Send + 'static>,
        > + Send
        + Sync,
>;

/// Host-side methods a sidecar may call (requests, unlike `event/notify`:
/// they carry a sidecar-originated **string** `id` and expect a
/// `{"id","result"}` reply). Host→sidecar ids stay numeric — the two
/// namespaces never collide, so replies route unambiguously.
pub const HOST_RUN: &str = "host/run";
pub const HOST_SAY: &str = "host/say";
/// Sidecar→host blocking user question (`params: {"questions","blocking"}`
/// → `result: {"answers"}`). Claimed implicitly: any sidecar whose manifest
/// has `protocol: "1.1"` may call it (the transport routes every `host/*`
/// method already); the extended `ASK_TTL` below applies per requesting
/// sidecar, keyed off its manifest.
pub const HOST_ASK: &str = "host/ask";

/// Default TTL for host→sidecar requests and plugin→host handler tasks.
/// `host/ask` is the only exception: a human answer takes minutes, so both
/// the handler task and the outer `tool/call`/`tool/before` wait use
/// [`ASK_TTL`] when the requesting sidecar's manifest claims `host/ask`.
pub const HOST_TTL: Duration = Duration::from_secs(30);
/// Outer deadline for `tool/call`/`tool/before` on sidecars that claim
/// `host/ask` (300s human answer + 30s transport slack). The sidecar must
/// enforce a SHORTER inner TTL (the reference plugins use 300s) so it
/// reports its own timeout instead of surfacing this generic one.
pub const ASK_TTL: Duration = Duration::from_secs(330);
/// TTL for one plugin→host `host/ask` handler task (same budget as ASK_TTL:
/// the modal owns the wait, this only bounds a wedged host task).
pub const ASK_HANDLER_TTL: Duration = Duration::from_secs(300);

/// Build the v1.1 `session` object: `{"id":<session_id or "">,"cwd":<cwd>}`.
/// `tool/call` uses `ctx` (cwd + session_id); every other wire point uses
/// the pinned boot cwd and `""` (no `ToolContext` there to read).
fn session_json(id: &str, cwd: &str) -> Value {
    json!({"id": id, "cwd": cwd})
}

/// In-flight request senders, keyed by request id. `epoch` marks the child
/// generation: a stale reader exiting late must not fail a new child's
/// requests (bumped on every respawn).
/// A torn frame poisons the child's stdin stream: no later request can be
/// framed correctly. Every write therefore gets its own bound — a TTL
/// expiry elsewhere must never cancel `write_all` mid-line, and a child
/// that stopped reading must not park a caller (or, in the host-reply
/// path, a concurrency permit) forever.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(windows)]
fn debug_log(message: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

/// Outcome of one attempt to hand a whole frame to the child.
enum FrameWrite {
    Ok,
    Failed(String),
    TimedOut,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestSensitivity {
    Normal,
    Sensitive,
}

#[cfg(windows)]
async fn try_write_frame(stdin: &std::sync::Arc<Mutex<ChildStdin>>, frame: &str) -> FrameWrite {
    // Tokio's Windows ChildStdin write is backed by a blocking pipe write.
    // A timeout cannot interrupt that operation while it is being polled, so
    // put the write in its own task and bound only the oneshot wait. The
    // caller can then terminate the child and let the detached task unwind.
    let stdin = Arc::clone(stdin);
    let frame = frame.to_owned();
    let guard = stdin.lock_owned().await;
    debug_log(&format!(
        "gray sidecar debug: spawning blocking write ({} bytes)",
        frame.len()
    ));
    let write = tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("sidecar write runtime builds");
        runtime.block_on(async move { guard.write_all(frame.as_bytes()).await })
    });
    match timeout(WRITE_TIMEOUT, write).await {
        Ok(Ok(Ok(()))) => FrameWrite::Ok,
        Ok(Ok(Err(e))) => FrameWrite::Failed(format!("{e}")),
        Ok(Err(e)) => FrameWrite::Failed(format!("sidecar stdin writer failed: {e}")),
        Err(_) => {
            debug_log("gray sidecar debug: blocking write timed out");
            FrameWrite::TimedOut
        }
    }
}

#[cfg(not(windows))]
async fn try_write_frame(stdin: &std::sync::Arc<Mutex<ChildStdin>>, frame: &str) -> FrameWrite {
    let mut guard = stdin.lock().await;
    match timeout(WRITE_TIMEOUT, guard.write_all(frame.as_bytes())).await {
        Ok(Ok(())) => FrameWrite::Ok,
        Ok(Err(e)) => FrameWrite::Failed(format!("{e}")),
        Err(_) => FrameWrite::TimedOut,
    }
}

struct Pending {
    epoch: u64,
    map: HashMap<u64, oneshot::Sender<Value>>,
}

/// Shared sidecar transport: child handle (respawn only), stdin (short
/// writer lock), pending replies, id counter, spawn argv.
struct Transport {
    child: Mutex<Child>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<Pending>>,
    next_id: AtomicU64,
    argv: Vec<String>,
    /// Plugin→host handler (`host/run`/`host/say`). `Arc` so respawned
    /// readers keep the same slot; `None` = reply `{"error":...}`.
    host_handler: Arc<Mutex<Option<HostHandler>>>,
    /// Bound on concurrent plugin→host handler tasks (shared across
    /// respawns so a respawn storm can't multiply it).
    host_slots: Arc<tokio::sync::Semaphore>,
    /// Capability ids the operator granted this plugin. Empty until the
    /// host records them, which is also the default: an ungranted
    /// `host/*` call is refused rather than trusted.
    grants: Arc<std::sync::Mutex<BTreeSet<String>>>,
}

pub struct SidecarPlugin {
    manifest: Manifest,
    tools: Vec<Arc<dyn Tool>>,
    transport: Arc<Transport>,
    /// Pinned boot cwd (captured at spawn from the process cwd). Used for
    /// the `session.cwd` of every wire point without a `ToolContext`
    /// (`prompt/context` uses its `cwd` arg instead; `tool/call` uses `ctx.cwd`).
    cwd: String,
    /// True when this sidecar may block on `host/ask` (manifest
    /// `protocol: "1.1"`): `tool/call`/`tool/before` get [`ASK_TTL`]
    /// instead of 30s. Protocol-gated (not hooks-gated): asking is a
    /// sidecar→host request, not a host→sidecar hook, so `claims()` does
    /// not apply. v1.1-only so a pre-v1 sidecar that never answers still
    /// fails fast at 30s.
    asks: bool,
}

/// ETXTBSY (os error 26) means the executable was open for writing at the
/// instant we exec'd it. A plugin binary or script written moments earlier
/// — a fresh install, a rebuild — is enough to trip it, and it is
/// transient by definition: nobody holds the file once the write lands.
/// One short retry turns a spurious "text file busy" into a non-event.
/// Measured on this box: a test that exec'd a just-written script failed
/// roughly 1 run in 5-20, with no process holding the file at the time.
/// `FnMut`, not `Fn`: the Windows branch configures a `Command` in place
/// (`.stdin()` takes `&mut self`), and a plain `Fn` closure cannot borrow
/// its capture mutably. That branch is `#[cfg(windows)]`, so only the
/// Windows CI job ever compiles it.
fn spawn_retrying_etxtbsy(
    mut spawn: impl FnMut() -> std::io::Result<Child>,
) -> std::io::Result<Child> {
    match spawn() {
        Err(e) if e.raw_os_error() == Some(26) => {
            std::thread::sleep(std::time::Duration::from_millis(25));
            spawn()
        }
        other => other,
    }
}

fn spawn_child(argv: &[String]) -> anyhow::Result<(Child, ChildStdin, ChildStdout)> {
    let (prog, args) = argv
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty argv"))?;
    // Windows cannot exec a shebang script directly (os error 193). Shell
    // plugins documented for Git Bash go through the same POSIX shell the
    // bash tool owns; native executables spawn directly as before.
    #[cfg(windows)]
    if prog.to_ascii_lowercase().ends_with(".sh") {
        let script = prog.replace('\\', "/");
        let mut cmd = Command::new(gray_tools::shell::shell_path()?);
        cmd.arg("-c")
            .arg("exec \"$1\"")
            .arg("sh")
            .arg(script)
            .args(args);
        let child = spawn_retrying_etxtbsy(|| {
            cmd.stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
        })?;
        return finish_spawn(child);
    }
    let child = spawn_retrying_etxtbsy(|| {
        Command::new(prog)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
    })?;
    finish_spawn(child)
}

fn finish_spawn(mut child: Child) -> anyhow::Result<(Child, ChildStdin, ChildStdout)> {
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdout"))?;
    Ok((child, stdin, stdout))
}

/// Route reply lines to their request by `id`; lines without a known `id`
/// are ignored. On EOF (child gone) fail only our own generation's
/// in-flight requests so callers report crash instead of hanging.
/// Sidecar-originated requests (`method: "host/..."` + **string** `id`)
/// dispatch to the host handler and get a `{"id","result"}` reply on the
/// same stdio; anything else without a pending numeric id is dropped.
/// Max wire frame: longer lines are skipped, never buffered whole.
const MAX_FRAME: usize = 256 * 1024;
/// Max concurrent plugin→host handler tasks; excess gets an overload
/// error instead of spawning unbounded tasks.
const MAX_HOST_TASKS: usize = 4;
/// Max in-flight requests per transport; excess fails fast.
const MAX_PENDING: usize = 64;

#[allow(clippy::too_many_arguments)]
fn spawn_reader(
    stdout: ChildStdout,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<Pending>>,
    host_handler: Arc<Mutex<Option<HostHandler>>>,
    host_slots: Arc<tokio::sync::Semaphore>,
    grants: Arc<std::sync::Mutex<BTreeSet<String>>>,
    // Plugin name, for the capability error the sidecar can act on.
    plugin_name: String,
    epoch: u64,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        // Consecutive over-cap chunks without a newline: a wedged child
        // streaming garbage. Break so the loop can't spin forever; pending
        // requests still fail via their own TTLs.
        let mut oversize_streak = 0u32;
        loop {
            let mut buf = Vec::new();
            let n = match (&mut reader)
                .take((MAX_FRAME + 1) as u64)
                .read_until(b'\n', &mut buf)
                .await
            {
                Ok(n) => n,
                Err(_) => break,
            };
            if n == 0 {
                break; // EOF
            }
            if n > MAX_FRAME {
                oversize_streak += 1;
                if oversize_streak > 64 {
                    break;
                }
                continue;
            }
            oversize_streak = 0;
            let Ok(v) = serde_json::from_slice::<Value>(&buf) else {
                continue;
            };
            // Plugin→host request: string id + host/ method.
            if let (Some(id), Some(method)) = (
                v.get("id").cloned(),
                v.get("method").and_then(|m| m.as_str()),
            ) && id.is_string()
                && method.starts_with("host/")
            {
                let params = v.get("params").cloned().unwrap_or(Value::Null);
                let method_owned = method.to_string();
                let stdin = stdin.clone();
                let host_handler = host_handler.clone();
                let host_slots = host_slots.clone();
                // Consent gate: every host/* method maps to one capability,
                // and the host only runs it when the operator granted it.
                let grants = grants.clone();
                let plugin_name = plugin_name.clone();
                // `grants` is written once, right after spawn, so this
                // lock is never contended; keeping it async avoids parking
                // a worker thread on the read path.
                let ungranted = {
                    let set = grants.lock().unwrap_or_else(|e| e.into_inner());
                    crate::capabilities::capability_for_host_method(&method_owned)
                        .filter(|cap| !set.contains(*cap))
                        .map(|cap| cap.to_string())
                };
                if let Some(cap) = ungranted {
                    // Fail closed with something the plugin can act on:
                    // the capability, and the command that grants it.
                    let reply = json!({"id": id, "result": {
                        "error": format!("capability_not_granted: {cap}"),
                        "hint": format!("gray plugin capabilities {plugin_name}"),
                    }});
                    if !matches!(
                        try_write_frame(&stdin, &format!("{reply}\n")).await,
                        FrameWrite::Ok
                    ) {
                        log::warn!("sidecar capability reply could not be written");
                    }
                    continue;
                }
                // Bounded handler tasks: at capacity, reply overload
                // instead of spawning unbounded work.
                let Ok(_permit) = host_slots.try_acquire_owned() else {
                    let reply = json!({"id": id, "result": {"error": "host overloaded"}});
                    // Bounded like every other frame: a wedged child must
                    // not park the read loop itself (it would stall every
                    // later reply behind the stuck one).
                    if !matches!(
                        try_write_frame(&stdin, &format!("{reply}\n")).await,
                        FrameWrite::Ok
                    ) {
                        log::warn!("sidecar overload reply could not be written");
                    }
                    continue;
                };
                // `host/ask` waits on a human: extended handler budget.
                // Everything else keeps the 30s TTL.
                let handler_ttl = if method_owned == HOST_ASK {
                    ASK_HANDLER_TTL
                } else {
                    HOST_TTL
                };
                tokio::spawn(async move {
                    let _permit = _permit;
                    let handler = host_handler.lock().await.clone();
                    let result = match handler {
                        Some(h) => timeout(handler_ttl, h(method_owned.clone(), params))
                            .await
                            .unwrap_or_else(
                                |_| json!({"error": format!("{method_owned} timed out")}),
                            ),
                        None => json!({"error": format!("no host handler for {method_owned}")}),
                    };
                    let reply = json!({"id": id, "result": result});
                    match try_write_frame(&stdin, &format!("{reply}\n")).await {
                        FrameWrite::Ok => {}
                        FrameWrite::Failed(e) => {
                            log::warn!("sidecar reply write failed ({e}); dropping the permit");
                        }
                        FrameWrite::TimedOut => {
                            // A wedged child holds the permit for nothing:
                            // end the task so the slot is reclaimed.
                            log::warn!("sidecar stopped reading stdin; dropping a host permit");
                        }
                    }
                });
                continue;
            }
            let Some(id) = v.get("id").and_then(|i| i.as_u64()) else {
                continue;
            };
            let tx = pending.lock().await.map.remove(&id);
            if let Some(tx) = tx {
                let _ = tx.send(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
        let mut p = pending.lock().await;
        if p.epoch == epoch {
            p.map.clear();
        }
    });
}

impl Transport {
    /// Best-effort plugin name for a capability error: the argv's basename
    /// without an extension, which is how sidecars are installed
    /// (`<home>/plugins/<name>/<bin>`).
    fn name_of(argv: &[String]) -> String {
        argv.first()
            .and_then(|p| Path::new(p).file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "this plugin".to_string())
    }

    fn new(child: Child, stdin: ChildStdin, stdout: ChildStdout, argv: Vec<String>) -> Arc<Self> {
        let pending = Arc::new(Mutex::new(Pending {
            epoch: 0,
            map: HashMap::new(),
        }));
        let stdin = Arc::new(Mutex::new(stdin));
        let host_handler = Arc::new(Mutex::new(None));
        let host_slots = Arc::new(tokio::sync::Semaphore::new(MAX_HOST_TASKS));
        let grants: Arc<std::sync::Mutex<BTreeSet<String>>> =
            Arc::new(std::sync::Mutex::new(BTreeSet::new()));
        spawn_reader(
            stdout,
            stdin.clone(),
            pending.clone(),
            host_handler.clone(),
            host_slots.clone(),
            grants.clone(),
            Self::name_of(&argv),
            0,
        );
        Arc::new(Self {
            child: Mutex::new(child),
            stdin,
            pending,
            next_id: AtomicU64::new(1),
            argv,
            host_handler,
            host_slots,
            grants,
        })
    }

    /// Respawn a dead child (new stdio + reader; old generation's in-flight
    /// requests fail fast). Lock order is always child → stdin → pending.
    async fn ensure_alive(&self) -> bool {
        let mut child = self.child.lock().await;
        if matches!(child.try_wait(), Ok(None)) {
            return true;
        }
        log::warn!(target: "gray_plugin", "sidecar child exited, respawning");
        match spawn_child(&self.argv) {
            Ok((new_child, stdin, stdout)) => {
                *child = new_child;
                *self.stdin.lock().await = stdin;
                let mut p = self.pending.lock().await;
                p.epoch += 1;
                p.map.clear();
                spawn_reader(
                    stdout,
                    self.stdin.clone(),
                    self.pending.clone(),
                    self.host_handler.clone(),
                    self.host_slots.clone(),
                    self.grants.clone(),
                    Self::name_of(&self.argv),
                    p.epoch,
                );
                true
            }
            Err(e) => {
                log::warn!(target: "gray_plugin", "sidecar respawn failed: {e}");
                false
            }
        }
    }

    /// Terminate the current child generation without waiting forever.
    ///
    /// Tokio's Windows child stdin uses a blocking pipe writer. If a shell
    /// script has spawned a grandchild, killing only the direct child leaves
    /// the pipe held open and the writer permanently blocked. `taskkill /T`
    /// closes that whole tree; the timeout keeps this best-effort cleanup
    /// from becoming a new lifecycle wait.
    async fn terminate_child_generation(&self) {
        let mut child = self.child.lock().await;
        #[cfg(windows)]
        if let Some(pid) = child.id() {
            #[cfg(windows)]
            debug_log(&format!("gray sidecar debug: taskkill pid={pid}"));
            let pid = pid.to_string();
            let mut taskkill = Command::new("taskkill");
            taskkill.kill_on_drop(true).args(["/PID", &pid, "/T", "/F"]);
            let _ = timeout(Duration::from_secs(2), taskkill.status()).await;
            #[cfg(windows)]
            debug_log("gray sidecar debug: taskkill returned");
        }
        let _ = child.start_kill();
        #[cfg(windows)]
        debug_log("gray sidecar debug: direct start_kill returned");
    }

    /// Fire-and-forget notification: write one `{"method","params"}` line
    /// (no `id`, no reply) with a 5s timeout. Shared by `event/notify` and
    /// `plugin/shutdown` — pre-v1 sidecars already ignore unknown lines.
    async fn send_notification(&self, method: &str, params: Value) -> bool {
        let req = json!({"method": method, "params": params});
        if !self.ensure_alive().await {
            return false;
        }
        // The write carries its own bound (WRITE_TIMEOUT): a torn
        // notification would desync every later frame on this stdin.
        matches!(
            try_write_frame(&self.stdin, &format!("{req}\n")).await,
            FrameWrite::Ok
        )
    }

    async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        ttl: Duration,
    ) -> anyhow::Result<Value> {
        self.request_with_sensitivity(method, params, ttl, RequestSensitivity::Normal)
            .await
    }

    async fn request_sensitive(
        &self,
        method: &str,
        params: Option<Value>,
        ttl: Duration,
    ) -> anyhow::Result<Value> {
        self.request_with_sensitivity(method, params, ttl, RequestSensitivity::Sensitive)
            .await
    }

    async fn request_with_sensitivity(
        &self,
        method: &str,
        params: Option<Value>,
        ttl: Duration,
        sensitivity: RequestSensitivity,
    ) -> anyhow::Result<Value> {
        let started = std::time::Instant::now();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut req = json!({"id": id, "method": method});
        if let Some(p) = params {
            req["params"] = p;
        }
        let frame = format!("{req}\n");
        if frame.len() > MAX_FRAME {
            let error = anyhow::anyhow!("sidecar request frame too large ({method})");
            if sensitivity == RequestSensitivity::Sensitive {
                log::debug!(target: "gray_plugin", "provider rpc method={} frame_bytes={} elapsed_ms={} outcome=error", method, frame.len(), started.elapsed().as_millis());
            }
            return Err(error);
        }
        // One deadline for the whole lifecycle (admission + write + reply):
        // a dead child, a stuck stdin lock, or a child that stops reading
        // must not wedge us past the advertised TTL. Insert the pending
        // entry after ensure_alive: a respawn clears the pending map.
        let outcome = timeout(ttl, async {
            if !self.ensure_alive().await {
                anyhow::bail!("sidecar child dead and respawn failed");
            }
            {
                let pending = self.pending.lock().await;
                if pending.map.len() >= MAX_PENDING {
                    anyhow::bail!("sidecar overloaded: too many in-flight requests");
                }
            }
            let (tx, rx) = oneshot::channel();
            self.pending.lock().await.map.insert(id, tx);
            match try_write_frame(&self.stdin, &frame).await {
                FrameWrite::Ok => {}
                FrameWrite::Failed(e) => {
                    #[cfg(windows)]
                    debug_log("gray sidecar debug: request failed-write branch");
                    self.pending.lock().await.map.remove(&id);
                    // Do not await child exit here: on Windows a shell-wrapped
                    // child may leave a grandchild alive, so `kill().await`
                    // can outlive the write timeout and wedge the caller.
                    self.terminate_child_generation().await;
                    anyhow::bail!("sidecar write failed ({e}); killed this child generation");
                }
                FrameWrite::TimedOut => {
                    #[cfg(windows)]
                    debug_log("gray sidecar debug: request timeout-write branch");
                    self.pending.lock().await.map.remove(&id);
                    // Kill the complete Windows process tree without
                    // waiting for the direct child to exit.
                    self.terminate_child_generation().await;
                    anyhow::bail!(
                        "sidecar stopped reading stdin; killed this child generation ({method})"
                    );
                }
            }
            rx.await
                .map_err(|_| anyhow::anyhow!("sidecar child closed stdout"))
        })
        .await;
        self.pending.lock().await.map.remove(&id);
        let result = match outcome {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("sidecar request timed out ({method})")),
        };
        if sensitivity == RequestSensitivity::Sensitive {
            log::debug!(target: "gray_plugin", "provider rpc method={} frame_bytes={} elapsed_ms={} outcome={}", method, frame.len(), started.elapsed().as_millis(), if result.is_ok() { "ok" } else { "error" });
        }
        result
    }
}

impl SidecarPlugin {
    pub async fn spawn(argv: Vec<String>) -> anyhow::Result<Self> {
        #[cfg(windows)]
        debug_log("gray sidecar debug: spawn start");
        let (child, stdin, stdout) = spawn_child(&argv)?;
        #[cfg(windows)]
        debug_log("gray sidecar debug: spawn child created");
        let transport = Transport::new(child, stdin, stdout, argv.clone());
        #[cfg(windows)]
        debug_log("gray sidecar debug: transport created; manifest request start");
        let result = transport
            .request("plugin/manifest", None, Duration::from_secs(30))
            .await?;
        #[cfg(windows)]
        debug_log("gray sidecar debug: manifest request returned");
        let mut manifest = Manifest::from_result(&result);
        let name = manifest.name.trim().to_string();
        if name.is_empty() {
            anyhow::bail!(
                "sidecar manifest has missing/empty name (argv: {})",
                argv.join(" ")
            );
        }
        manifest.name = name;
        let asks = manifest.protocol.as_deref() == Some("1.1");
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        for entry in manifest_tools(&result) {
            tools.push(Arc::new(SidecarTool::new(entry, transport.clone(), asks)));
        }
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| ".".to_string());
        Ok(Self {
            manifest,
            tools,
            transport,
            cwd,
            asks,
        })
    }

    /// Graceful lifecycle teardown: send `plugin/shutdown` (`reason:
    /// `session_end`) to v1.1 sidecars only (pre-v1 `protocol: None` never
    /// receives the line — they ignore unknown input anyway), then wait up
    /// to `grace` for the child to exit voluntarily and `kill()` what remains.
    /// Never waits for a reply (notification has no `id`).
    pub async fn shutdown(&self, grace: Duration) {
        if self.manifest.protocol.is_some() {
            let _ = self
                .transport
                .send_notification("plugin/shutdown", json!({"reason": "session_end"}))
                .await;
        }
        let mut child = self.transport.child.lock().await;
        let _ = timeout(grace, child.wait()).await;
        let _ = child.kill().await;
    }
    /// Install the plugin→host handler (`host/run`/`host/say`). The host
    /// sets this once after spawn; sidecar requests then dispatch to it
    /// with a 30s TTL each. Without a handler the transport replies
    /// `{"error":...}` (loud failure, never a hang).
    /// Record the capabilities the operator granted this plugin. Called
    /// by the host right after spawn; until it is called the plugin has
    /// none, which is also the correct default for a sidecar nobody asked
    /// about.
    pub fn set_capabilities(&self, granted: Vec<String>) {
        let mut set = self
            .transport
            .grants
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *set = granted
            .into_iter()
            .filter(|c| !c.trim().is_empty())
            .collect();
    }

    /// Capabilities this sidecar currently holds.
    pub fn capabilities(&self) -> Vec<String> {
        self.transport
            .grants
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    fn require_provider_credentials(&self) -> Result<(), ProviderRpcError> {
        if self
            .capabilities()
            .iter()
            .any(|capability| capability == PROVIDER_CREDENTIALS)
        {
            Ok(())
        } else {
            Err(ProviderRpcError::CapabilityMissing(
                PROVIDER_CREDENTIALS.to_owned(),
            ))
        }
    }

    async fn provider_rpc(
        &self,
        method: &str,
        params: Value,
        ttl: Duration,
    ) -> Result<Value, ProviderRpcError> {
        self.require_provider_credentials()?;
        let value = self
            .transport
            .request_sensitive(method, Some(params), ttl)
            .await
            .map_err(|error| ProviderRpcError::Unavailable(error.to_string()))?;
        if let Some(error) = value.get("error") {
            let failure = serde_json::from_value(error.clone()).map_err(|_| {
                ProviderRpcError::Protocol("provider RPC returned an invalid error".into())
            })?;
            return Err(ProviderRpcError::Rpc(failure));
        }
        Ok(value)
    }

    pub async fn provider_auth_start(
        &self,
        provider: &str,
        auth_method: &str,
    ) -> Result<ProviderAuthStart, ProviderRpcError> {
        let value = self
            .provider_rpc(
                "provider/auth/start",
                json!({"provider": provider, "auth_method": auth_method}),
                Duration::from_secs(10),
            )
            .await?;
        serde_json::from_value(value)
            .map_err(|_| ProviderRpcError::Protocol("invalid provider auth start".into()))
    }

    pub async fn provider_auth_poll(
        &self,
        operation_id: &str,
    ) -> Result<ProviderAuthPoll, ProviderRpcError> {
        let value = self
            .provider_rpc(
                "provider/auth/poll",
                json!({"operation_id": operation_id}),
                Duration::from_secs(10),
            )
            .await?;
        parse_auth_poll(value)
    }

    pub async fn provider_auth_cancel(&self, operation_id: &str) -> Result<(), ProviderRpcError> {
        self.provider_rpc(
            "provider/auth/cancel",
            json!({"operation_id": operation_id}),
            Duration::from_secs(10),
        )
        .await?;
        Ok(())
    }

    pub async fn provider_auth_refresh(
        &self,
        request: &ProviderRefreshRequest,
    ) -> Result<CredentialMaterial, ProviderRpcError> {
        let params = serde_json::to_value(request)
            .map_err(|_| ProviderRpcError::Protocol("invalid provider refresh request".into()))?;
        let value = self
            .provider_rpc("provider/auth/refresh", params, Duration::from_secs(30))
            .await?;
        parse_credential_material(value)
    }

    pub async fn provider_auth_revoke(
        &self,
        request: &ProviderRevokeRequest,
    ) -> Result<ProviderRevokeResult, ProviderRpcError> {
        let params = serde_json::to_value(request)
            .map_err(|_| ProviderRpcError::Protocol("invalid provider revoke request".into()))?;
        let value = self
            .provider_rpc("provider/auth/revoke", params, Duration::from_secs(10))
            .await?;
        match value.get("status").and_then(Value::as_str) {
            Some("revoked") => Ok(ProviderRevokeResult::Revoked),
            Some("unsupported") => Ok(ProviderRevokeResult::Unsupported),
            _ => match serde_json::from_value(value) {
                Ok(result) => Ok(result),
                Err(_) => Err(ProviderRpcError::Protocol(
                    "invalid provider revoke result".into(),
                )),
            },
        }
    }

    pub async fn provider_models(
        &self,
        request: &ProviderModelsRequest,
    ) -> Result<ProviderModelCatalog, ProviderRpcError> {
        let params = serde_json::to_value(request)
            .map_err(|_| ProviderRpcError::Protocol("invalid provider models request".into()))?;
        let value = self
            .provider_rpc("provider/models", params, Duration::from_secs(30))
            .await?;
        serde_json::from_value(value)
            .map_err(|_| ProviderRpcError::Protocol("invalid provider model catalog".into()))
    }

    pub async fn set_host_handler(&self, handler: HostHandler) {
        *self.transport.host_handler.lock().await = Some(handler);
    }
    /// v1 request methods are gated on the manifest's `hooks`/`commands`:
    /// pre-v1 sidecars ignore unknown lines and would never reply, so
    /// sending them anything new would hang every turn to the full timeout.
    fn claims(&self, hook: &str) -> bool {
        self.manifest.hooks.iter().any(|h| h == hook)
    }
}

fn parse_auth_poll(value: Value) -> Result<ProviderAuthPoll, ProviderRpcError> {
    let state = value
        .get("state")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderRpcError::Protocol("invalid provider auth state".into()))?;
    match state {
        "pending" => Ok(ProviderAuthPoll::Pending {
            retry_after_ms: value.get("retry_after_ms").and_then(Value::as_u64),
        }),
        "completed" => {
            let credential = value
                .get("credential")
                .cloned()
                .ok_or_else(|| ProviderRpcError::Protocol("invalid provider auth state".into()))?;
            Ok(ProviderAuthPoll::Completed(parse_credential_material(
                credential,
            )?))
        }
        "failed" => {
            let failure = value
                .get("error")
                .cloned()
                .ok_or_else(|| ProviderRpcError::Protocol("invalid provider auth state".into()))?;
            serde_json::from_value(failure)
                .map(ProviderAuthPoll::Failed)
                .map_err(|_| ProviderRpcError::Protocol("invalid provider auth state".into()))
        }
        "cancelled" => Ok(ProviderAuthPoll::Cancelled),
        "operation_lost" => Ok(ProviderAuthPoll::OperationLost),
        _ => Err(ProviderRpcError::Protocol(
            "invalid provider auth state".into(),
        )),
    }
}

fn parse_credential_material(value: Value) -> Result<CredentialMaterial, ProviderRpcError> {
    let value = value.get("credential").cloned().unwrap_or(value);
    serde_json::from_value(value)
        .map_err(|_| ProviderRpcError::Protocol("invalid provider credential".into()))
}

#[async_trait]
impl Plugin for SidecarPlugin {
    /// Granted capabilities, for tool-assembly checks that run outside
    /// async (`builder::from_plugins`).
    fn capabilities(&self) -> Vec<String> {
        SidecarPlugin::capabilities(self)
    }

    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.clone()
    }
    async fn prompt_context(&self, cwd: &str) -> Option<String> {
        if !self.claims("prompt/context") {
            return None;
        }
        let params = json!({"cwd": cwd, "session": session_json("", cwd)});
        let v = self
            .transport
            .request("prompt/context", Some(params), Duration::from_secs(30))
            .await
            .ok()?;
        v.get("text")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
    }
    async fn tool_before(&self, name: &str, args: &Value) -> ToolBefore {
        if !self.claims("tool/before") {
            return ToolBefore::Allow;
        }
        let params = json!({"name": name, "args": args, "session": session_json("", &self.cwd)});
        let ttl = if self.asks { ASK_TTL } else { HOST_TTL };
        match self
            .transport
            .request("tool/before", Some(params), ttl)
            .await
        {
            Ok(v) => ToolBefore::from_result(&v),
            Err(e) => {
                log::warn!(target: "gray_plugin", "sidecar tool/before failed: {e}");
                ToolBefore::Deny("plugin policy unavailable; tool not executed".to_string())
            }
        }
    }
    async fn run_command(&self, name: &str, argv: Vec<String>) -> Option<CommandOutcome> {
        // `subcommands` (cron, …) forward argv over the same
        // `command/run` wire as `commands` — one path, no special-casing.
        if !self.manifest.commands.iter().any(|c| c == name)
            && !self.manifest.subcommands.iter().any(|c| c == name)
        {
            return None;
        }
        let params = json!({"name": name, "argv": argv, "session": session_json("", &self.cwd)});
        let v = self
            .transport
            .request("command/run", Some(params), Duration::from_secs(30))
            .await
            .ok()?;
        // `{"prompt":...}` wins over `{"text":...}`; empty/missing → None.
        if let Some(p) = v
            .get("prompt")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
        {
            return Some(CommandOutcome::Prompt(p.to_string()));
        }
        v.get("text")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .map(|t| CommandOutcome::Say(t.to_string()))
    }
    async fn on_event(&self, e: CoreEvent) {
        // Minimal tagged JSON (see protocol v1 doc comment above) + v1.1 session.
        let session = session_json("", &self.cwd);
        let params = match &e {
            CoreEvent::PreTool { name, args } => {
                json!({"type": "pre_tool", "name": name, "args": args, "session": session})
            }
            CoreEvent::PostTool { name, output } => {
                json!({"type": "post_tool", "name": name, "content": output.content, "is_error": output.is_error, "session": session})
            }
            CoreEvent::TurnEnd { usage } => {
                json!({"type": "turn_end", "usage": usage, "session": session})
            }
        };
        let name = self.manifest.name.clone();
        // True notification via shared helper: no id, never a reply.
        if !self
            .transport
            .send_notification("event/notify", params)
            .await
        {
            log::warn!(target: "gray_plugin", "sidecar {name} hook failed, skipping");
        }
    }
    async fn shutdown(&self) {
        self.shutdown(Duration::from_secs(2)).await;
    }
}

struct SidecarTool {
    def: ToolDef,
    transport: Arc<Transport>,
    /// Snapshot of the owning sidecar's `asks` flag (see [`SidecarPlugin`]).
    asks: bool,
}

impl SidecarTool {
    fn new(def: ToolDef, transport: Arc<Transport>, asks: bool) -> Self {
        Self {
            def,
            transport,
            asks,
        }
    }

    fn transport_asks(&self) -> bool {
        self.asks
    }
}

#[async_trait]
impl Tool for SidecarTool {
    fn def(&self) -> ToolDef {
        self.def.clone()
    }
    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let name = self.def.name.clone();
        let cwd = ctx.cwd.to_string_lossy();
        let sid = ctx.session_id.as_deref().unwrap_or("");
        let params = json!({"name": name, "args": args, "session": session_json(sid, &cwd)});
        let ttl = if self.transport_asks() {
            ASK_TTL
        } else {
            HOST_TTL
        };
        match self.transport.request("tool/call", Some(params), ttl).await {
            Ok(v) => {
                // Route through the shared truncation (50 KiB cap with
                // annotation): raw sidecar content must not bypass it.
                let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false);
                let content = v.get("content").and_then(|c| c.as_str());
                match (is_error, content) {
                    (true, c) => gray_core::tool_out::fail(c.unwrap_or_default().to_string()),
                    (false, Some(c)) => gray_core::tool_out::finish(c.to_string()),
                    // No empty-success fallback: a reply without content is a
                    // protocol error, not a tool that returned nothing.
                    (false, None) => ToolOutput::error(format!(
                        "plugin protocol error: {name} reply missing content"
                    )),
                }
            }
            Err(e) => {
                log::warn!(target: "gray_plugin", "sidecar {name} tool call failed, skipping: {e}");
                let msg = e.to_string();
                let kind = if msg.contains("timed out") || msg.contains("elapsed") {
                    "timeout"
                } else if msg.contains("closed stdout") || msg.contains("respawn failed") {
                    "crashed"
                } else {
                    "protocol error"
                };
                ToolOutput::error(format!("plugin {kind}: {name}"))
            }
        }
    }
}

#[path = "sidecar_tests.rs"]
#[cfg(test)]
mod tests;
