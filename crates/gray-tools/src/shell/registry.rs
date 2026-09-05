//! shell/registry.rs — session-scoped process registry (brief 2A, Phase 2).
//!
//! Process-wide map of tasks keyed by session id. Ids are per-session
//! sequential (`t1`, `t2`, …) and never reused within a session.
//!
//! Until brief 2B wires it in, `tools/bash.rs` still issues ids from its
//! `AtomicU32` fallback; this module compiles mostly unreferenced (same
//! pattern as 1A/1B/1C before 1D).
//!
//! Locking: one `std::sync::Mutex`, never held across `.await`. Watch and
//! broadcast senders are cloned out under the lock and used outside it.

#![deny(clippy::await_holding_lock)]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::process::Child;
use tokio::sync::{broadcast, watch};

use super::contract::{
    EXITED_TASK_KEEP, EXITED_TASK_TTL, ExitReport, TaskId, TaskInfo, TaskState, WakeEvent,
};

const WAKE_CAP: usize = 256;
const USER_INPUT_COALESCE: Duration = Duration::from_millis(250);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

static REGISTRY: OnceLock<ProcessRegistry> = OnceLock::new();

/// Process-wide registry. Session key is
/// `ctx.session_id.clone().unwrap_or_else(|| "nosession".into())`.
pub fn registry() -> &'static ProcessRegistry {
    REGISTRY.get_or_init(ProcessRegistry::new)
}

pub struct ProcessRegistry {
    inner: Mutex<Inner>,
}

struct Inner {
    sessions: HashMap<String, SessionTasks>,
    wake: broadcast::Sender<WakeEvent>, // cap 256
    last_input: Option<Instant>,
}

struct SessionTasks {
    next: u32,
    // Keyed by raw id: contract's TaskId has no Ord and the contract is
    // frozen, so ordering lives here. Iteration is id-sorted via BTreeMap.
    tasks: BTreeMap<u32, Task>,
}

#[allow(dead_code)] // start_ticks lands with 2D, pattern_strikes with 3C
struct Task {
    info: TaskInfo,
    start_ticks: Option<u64>,
    bytes: watch::Sender<u64>,
    exit: watch::Sender<Option<ExitReport>>,
    pattern_strikes: u8,
}

impl ProcessRegistry {
    fn new() -> Self {
        let (wake, _) = broadcast::channel(WAKE_CAP);
        Self {
            inner: Mutex::new(Inner {
                sessions: HashMap::new(),
                wake,
                last_input: None,
            }),
        }
    }

    /// Register a just-spawned child. Ids are per-session sequential from t1,
    /// never reused within a session (even across processes this stays
    /// per-process until the REPL owns cross-process ids — known gap).
    pub fn register(
        &self,
        session: &str,
        child: &Child,
        command: &str,
        log_path: PathBuf,
    ) -> TaskId {
        let pid = child
            .id()
            .expect("registry::register: child has no pid");
        // All our spawns use setsid, so the child is its own group leader.
        let pgid = pid as i32;
        let start_ticks = start_ticks_for(pid);
        let started = Instant::now();
        let (bytes_tx, _) = watch::channel(0u64);
        let (exit_tx, _) = watch::channel(None);
        let mut guard = self.inner.lock().expect("registry lock poisoned");
        let sess = guard.sessions.entry(session.to_string()).or_insert_with(|| {
            SessionTasks {
                next: 1,
                tasks: BTreeMap::new(),
            }
        });
        let id = TaskId(sess.next);
        sess.next += 1;
        sess.tasks.insert(
            id.0,
            Task {
                info: TaskInfo {
                    id,
                    pid,
                    pgid,
                    command: command.to_string(),
                    started,
                    log_path,
                    bytes: 0,
                    state: TaskState::Running,
                },
                start_ticks,
                bytes: bytes_tx,
                exit: exit_tx,
                pattern_strikes: 0,
            },
        );
        id
    }

    pub fn get(&self, session: &str, id: TaskId) -> Option<TaskInfo> {
        let guard = self.inner.lock().expect("registry lock poisoned");
        guard
            .sessions
            .get(session)?
            .tasks
            .get(&id.0)
            .map(clone_info)
    }

    /// Sorted by id (BTreeMap order). Empty vec when the session is unknown.
    pub fn list(&self, session: &str) -> Vec<TaskInfo> {
        let guard = self.inner.lock().expect("registry lock poisoned");
        guard
            .sessions
            .get(session)
            .map(|s| s.tasks.values().map(clone_info).collect())
            .unwrap_or_default()
    }

    pub fn mark_exited(&self, session: &str, id: TaskId, report: ExitReport) {
        let (exit_tx, wake_tx) = {
            let mut guard = self.inner.lock().expect("registry lock poisoned");
            let Some(sess) = guard.sessions.get_mut(session) else {
                return;
            };
            let Some(task) = sess.tasks.get_mut(&id.0) else {
                return;
            };
            task.info.state = TaskState::Exited {
                report: report.clone(),
                at: Instant::now(),
            };
            (task.exit.clone(), guard.wake.clone())
        };
        // Zero receivers is normal (e.g. `-p` mode) — swallow SendError.
        let _ = exit_tx.send(Some(report.clone()));
        let _ = wake_tx.send(WakeEvent::Exited { id, report });
        self.gc(session);
    }

    pub fn bytes_rx(&self, session: &str, id: TaskId) -> Option<watch::Receiver<u64>> {
        let guard = self.inner.lock().expect("registry lock poisoned");
        guard
            .sessions
            .get(session)?
            .tasks
            .get(&id.0)
            .map(|t| t.bytes.subscribe())
    }

    pub fn exit_rx(
        &self,
        session: &str,
        id: TaskId,
    ) -> Option<watch::Receiver<Option<ExitReport>>> {
        let guard = self.inner.lock().expect("registry lock poisoned");
        guard
            .sessions
            .get(session)?
            .tasks
            .get(&id.0)
            .map(|t| t.exit.subscribe())
    }

    pub fn wake_tx(&self) -> broadcast::Sender<WakeEvent> {
        self.inner.lock().expect("registry lock poisoned").wake.clone()
    }

    /// Broadcast `UserInput`, coalesced: ignored when the last send was
    /// < 250 ms ago.
    pub fn notify_user_input(&self) {
        let wake = {
            let mut guard = self.inner.lock().expect("registry lock poisoned");
            let now = Instant::now();
            if guard
                .last_input
                .is_some_and(|t| now.saturating_duration_since(t) < USER_INPUT_COALESCE)
            {
                return;
            }
            guard.last_input = Some(now);
            guard.wake.clone()
        };
        let _ = wake.send(WakeEvent::UserInput);
    }

    /// Drop finished tasks: older than `EXITED_TASK_TTL` first, then oldest
    /// beyond `EXITED_TASK_KEEP`. Running tasks are never dropped. Log files
    /// are NOT deleted (the model may still grep them; a 7-day sweep owned
    /// by the REPL crate handles disk — note for 2E).
    pub fn gc(&self, session: &str) {
        let mut guard = self.inner.lock().expect("registry lock poisoned");
        let Some(sess) = guard.sessions.get_mut(session) else {
            return;
        };
        let now = Instant::now();
        sess.tasks.retain(|_, t| match &t.info.state {
            TaskState::Running => true,
            TaskState::Exited { at, .. } => {
                now.saturating_duration_since(*at) <= EXITED_TASK_TTL
            }
        });
        let mut exited: Vec<(Instant, u32)> = sess
            .tasks
            .iter()
            .filter_map(|(&id, t)| match &t.info.state {
                TaskState::Exited { at, .. } => Some((*at, id)),
                TaskState::Running => None,
            })
            .collect();
        if exited.len() > EXITED_TASK_KEEP {
            exited.sort();
            let drop_n = exited.len() - EXITED_TASK_KEEP;
            for (_, id) in exited.iter().take(drop_n) {
                sess.tasks.remove(id);
            }
        }
    }

    /// SIGTERM each Running task's group, 2 s grace, SIGKILL (inline until
    /// 2D's `kill::term_then_kill` lands), then drop the session.
    pub async fn shutdown_session(&self, session: &str) {
        let running: Vec<(u32, i32)> = {
            let guard = self.inner.lock().expect("registry lock poisoned");
            guard
                .sessions
                .get(session)
                .map(|s| {
                    s.tasks
                        .values()
                        .filter_map(|t| match t.info.state {
                            TaskState::Running => Some((t.info.pid, t.info.pgid)),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        for (pid, pgid) in running {
            signal_group(pgid, pid).await;
        }
        self.inner
            .lock()
            .expect("registry lock poisoned")
            .sessions
            .remove(session);
    }
}

/// Clone a snapshot; `bytes` is read live from the watch channel so headers
/// and `/tasks` never go stale (2B's pump writes through this sender).
fn clone_info(task: &Task) -> TaskInfo {
    let bytes = *task.bytes.borrow();
    let state = match &task.info.state {
        TaskState::Running => TaskState::Running,
        TaskState::Exited { report, at } => TaskState::Exited {
            report: report.clone(),
            at: *at,
        },
    };
    TaskInfo {
        id: task.info.id,
        pid: task.info.pid,
        pgid: task.info.pgid,
        command: task.info.command.clone(),
        started: task.info.started,
        log_path: task.info.log_path.clone(),
        bytes,
        state,
    }
}

/// Inline group kill until 2D lands: refuse degenerate/own groups, SIGTERM,
/// poll every 100 ms via `kill(-pgid, 0)`, SIGKILL after grace.
async fn signal_group(pgid: i32, _pid: u32) {
    if pgid <= 1 || pgid == std::process::id() as i32 {
        return;
    }
    let own_pgrp = unsafe { libc::getpgrp() };
    if pgid == own_pgrp {
        return;
    }
    unsafe {
        libc::kill(-pgid, libc::SIGTERM);
    }
    let t0 = Instant::now();
    loop {
        let alive = unsafe { libc::kill(-pgid, 0) } == 0;
        if !alive {
            return;
        }
        if t0.elapsed() >= SHUTDOWN_GRACE {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if unsafe { libc::kill(-pgid, 0) } == 0 {
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

/// Linux `/proc/{pid}/stat` field 22 (starttime); `None` elsewhere.
/// Duplicated from `spawn.rs` (2D owns the shared helper, if any).
#[cfg(target_os = "linux")]
fn start_ticks_for(pid: u32) -> Option<u64> {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()?
        .rsplit(')')
        .next()?
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn start_ticks_for(_pid: u32) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SESS_N: AtomicU64 = AtomicU64::new(0);

    fn sess(tag: &str) -> String {
        format!(
            "regtest-{tag}-{}-{}",
            std::process::id(),
            SESS_N.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn true_child() -> Child {
        tokio::process::Command::new("true")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn true")
    }

    fn ok_report() -> ExitReport {
        ExitReport {
            code: Some(0),
            signal: None,
            effective: 0,
            label: "exit 0".to_string(),
            note: None,
            benign: false,
        }
    }

    #[tokio::test]
    async fn concurrent_sessions_get_own_t1_t2() {
        let reg = std::sync::Arc::new(ProcessRegistry::new());
        let a = sess("conc-a");
        let b = sess("conc-b");
        let mut handles = Vec::new();
        for s in [a.clone(), b.clone(), a.clone(), b.clone()] {
            let r = reg.clone();
            handles.push(tokio::spawn(async move {
                let mut child = true_child();
                let id = r.register(&s, &child, "true", PathBuf::from("/tmp/x.log"));
                let _ = child.wait().await;
                (s, id)
            }));
        }
        let mut a_ids = Vec::new();
        let mut b_ids = Vec::new();
        for h in handles {
            let (s, id) = h.await.expect("join");
            if s == a {
                a_ids.push(id.0);
            } else {
                b_ids.push(id.0);
            }
        }
        a_ids.sort();
        b_ids.sort();
        assert_eq!(a_ids, vec![1, 2]);
        assert_eq!(b_ids, vec![1, 2]);
        assert_eq!(reg.list(&a).len(), 2);
        assert_eq!(reg.list(&b).len(), 2);
        // No leak across sessions.
        assert!(reg.list(&a).iter().all(|t| reg.get(&b, t.id).is_none()));
    }

    #[tokio::test]
    async fn mark_exited_resolves_watch_and_single_wake() {
        let reg = ProcessRegistry::new();
        let s = sess("exit");
        let mut child = true_child();
        let id = reg.register(&s, &child, "true", PathBuf::from("/tmp/x.log"));
        let _ = child.wait().await;
        let mut exit_rx = reg.exit_rx(&s, id).expect("exit_rx");
        let mut wake_rx = reg.wake_tx().subscribe();
        assert!(exit_rx.borrow().is_none());
        reg.mark_exited(&s, id, ok_report());
        tokio::time::timeout(Duration::from_secs(2), exit_rx.changed())
            .await
            .expect("exit watch resolves")
            .expect("changed ok");
        assert_eq!(exit_rx.borrow().as_ref().expect("report").effective, 0);
        match tokio::time::timeout(Duration::from_secs(2), wake_rx.recv())
            .await
            .expect("wake arrives")
            .expect("recv ok")
        {
            WakeEvent::Exited { id: got, .. } => assert_eq!(got, id),
            other => panic!("expected Exited, got {other:?}"),
        }
        assert!(wake_rx.try_recv().is_err(), "exactly one wake");
    }

    #[tokio::test]
    async fn gc_keeps_20_and_drops_ttl() {
        let reg = ProcessRegistry::new();
        let s = sess("gc");
        for _ in 0..25 {
            let mut child = true_child();
            let id = reg.register(&s, &child, "true", PathBuf::from("/tmp/x.log"));
            let _ = child.wait().await;
            reg.mark_exited(&s, id, ok_report());
        }
        // mark_exited auto-gcs; newest 20 survive.
        assert_eq!(reg.list(&s).len(), EXITED_TASK_KEEP);
        // Age one survivor 31 min back (inject clock via state) → gc drops it.
        let victim = reg.list(&s)[0].id;
        {
            let mut guard = reg.inner.lock().expect("poisoned");
            let task = guard.sessions.get_mut(&s).expect("sess").tasks.get_mut(&victim.0).expect("task");
            let TaskState::Exited { report, at } = &mut task.info.state else {
                panic!("expected exited");
            };
            *at = Instant::now() - Duration::from_secs(31 * 60);
            let _ = report;
        }
        reg.gc(&s);
        assert_eq!(reg.list(&s).len(), EXITED_TASK_KEEP - 1);
        assert!(reg.get(&s, victim).is_none());
    }

    #[tokio::test]
    async fn notify_user_input_coalesces() {
        let reg = ProcessRegistry::new();
        let mut rx = reg.wake_tx().subscribe();
        reg.notify_user_input();
        reg.notify_user_input();
        reg.notify_user_input();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 1, "3 rapid notifies → one broadcast");
        tokio::time::sleep(Duration::from_millis(300)).await;
        reg.notify_user_input();
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("second window delivers")
            .expect("recv ok");
    }

    #[tokio::test]
    async fn shutdown_session_kills_group_and_removes_session() {
        let reg = ProcessRegistry::new();
        let s = sess("shutdown");
        let log = std::env::temp_dir().join(format!("gray-regtest-{}-shutdown.log", std::process::id()));
        let spawned = crate::shell::spawn::spawn("sleep 30", &std::env::temp_dir(), TaskId(999))
            .expect("spawn sleep 30");
        let pid = spawned.pid;
        let mut child = spawned.child;
        let id = reg.register(&s, &child, "sleep 30", log);
        assert!(matches!(reg.get(&s, id).map(|t| t.state), Some(TaskState::Running)));
        tokio::time::timeout(Duration::from_secs(5), reg.shutdown_session(&s))
            .await
            .expect("shutdown returns");
        assert!(reg.get(&s, id).is_none());
        assert!(reg.list(&s).is_empty());
        let status = tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .expect("child reaped within 3 s")
            .expect("wait ok");
        // Killed by us, not a clean exit 0.
        assert_ne!(status.code(), Some(0), "sleep 30 must not exit 0 (pid {pid})");
    }

    #[test]
    fn global_registry_inits() {
        let _ = registry().wake_tx();
    }
}
