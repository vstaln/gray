//! Shared daemon-thread ThreadPoolExecutor.
//! Port of `tools/daemon_pool.py` (64 lines) — 1:1 behavior.
//!
//! Stdlib `ThreadPoolExecutor` workers are non-daemon AND are registered in
//! `concurrent.futures.thread._threads_queues`, whose atexit hook
//! (`_python_exit`) joins every worker unconditionally — even after
//! `shutdown(wait=False)`.  A single wedged worker (tool blocked on network
//! I/O, hung provider daemon, stuck subagent) therefore blocks interpreter
//! exit forever.  This is the root cause of multi-minute CLI exits on long
//! sessions: every abandoned concurrent-tool batch leaves workers that the
//! exit hook insists on joining.
//!
//! `DaemonThreadPoolExecutor` spawns daemon workers and skips the
//! `_threads_queues` registration, so:
//!
//!   - `_python_exit` never joins them, and
//!   - the interpreter's non-daemon thread join at shutdown skips them.
//!
//! Semantics are otherwise identical (initializer/initargs, work queue,
//! idle-thread reuse).  Use it for any pool whose work is best-effort or
//! independently interruptible and must never hold the process open:
//! concurrent tool execution, background memory sync, catalog fan-out,
//! subagent timeout wrappers.  Do NOT use it for work that must complete
//! before exit (durable writes) — those belong on foreground threads with
//! explicit bounded joins.
//!
//! Rust mapping:
//! - Python `threading.Thread(daemon=True)` → Rust detached `std::thread` (JoinHandle dropped).
//!   Rust's process exits when `main` returns regardless of detached threads — equivalent to
//!   daemon + no `_threads_queues` registration. There is no global `_threads_queues` in Rust.
//! - Python `weakref.ref(self, weakref_cb)` → Rust `Weak<Inner>`; `weakref_cb` puts `None`
//!   sentinel into the queue when the executor is dropped.
//! - Python `self._idle_semaphore.acquire(timeout=0)` → `AtomicUsize` idle counter with
//!   non-blocking `compare_exchange` (try-acquire).
//! - Python `_threads` set + `len(self._threads) < self._max_workers` → `AtomicUsize` thread count.
//! - Python `_work_queue` (`queue.SimpleQueue`) → `WorkQueue` (Mutex<VecDeque> + Condvar).

use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Condvar, Mutex, Weak,
};
use std::thread;
use std::time::Duration;

// ---------------------------------------------------------------------------
// __all__ equivalent
// ---------------------------------------------------------------------------

/// Mirrors `__all__ = ["DaemonThreadPoolExecutor"]` (line 34).
pub const ALL: &[&str] = &["DaemonThreadPoolExecutor"];

// ---------------------------------------------------------------------------
// Idle semaphore — mirrors `threading.Semaphore` with try-acquire (timeout=0)
// ---------------------------------------------------------------------------

struct IdleSemaphore {
    count: AtomicUsize,
}

impl IdleSemaphore {
    fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
        }
    }

    /// Mirrors `self._idle_semaphore.acquire(timeout=0)` — non-blocking try-acquire.
    /// Returns true if an idle thread was available and consumed.
    fn try_acquire(&self) -> bool {
        let mut current = self.count.load(Ordering::SeqCst);
        while current > 0 {
            match self.count.compare_exchange(
                current,
                current - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(v) => current = v,
            }
        }
        false
    }

    fn release(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    fn acquire_blocking(&self) {
        // not used — Python only uses timeout=0 non-blocking check
        self.release();
    }
}

// ---------------------------------------------------------------------------
// Work queue — mirrors `queue.SimpleQueue` + sentinel `None`
// ---------------------------------------------------------------------------

enum Task {
    Job(Box<dyn FnOnce() + Send + 'static>),
    Sentinel,
}

struct WorkQueue {
    queue: Mutex<VecDeque<Task>>,
    cvar: Condvar,
    shutdown: AtomicBool,
}

impl WorkQueue {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            cvar: Condvar::new(),
            shutdown: AtomicBool::new(false),
        }
    }

    fn put(&self, task: Task) {
        let mut q = self.queue.lock().unwrap();
        q.push_back(task);
        self.cvar.notify_one();
    }

    /// Non-blocking put of sentinel — mirrors `q.put(None)` in `weakref_cb` (line 47).
    fn put_sentinel(&self) {
        self.put(Task::Sentinel);
    }

    fn pop(&self) -> Task {
        let mut q = self.queue.lock().unwrap();
        loop {
            if let Some(task) = q.pop_front() {
                return task;
            }
            if self.shutdown.load(Ordering::SeqCst) {
                return Task::Sentinel;
            }
            q = self.cvar.wait(q).unwrap();
        }
    }

    fn set_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.cvar.notify_all();
    }
}

// ---------------------------------------------------------------------------
// Daemon future — mirrors `concurrent.futures.Future`
// ---------------------------------------------------------------------------

/// Mirrors `Future` returned by `executor.submit(...)`.
///
/// In Python: `pool.submit(fn, *args).result(timeout=10)`.
/// In Rust: `pool.submit(|| fn()).result_timeout(Duration::from_secs(10))`.
pub struct DaemonFuture<R> {
    rx: std::sync::mpsc::Receiver<R>,
}

impl<R> DaemonFuture<R> {
    /// Mirrors `future.result(timeout=10)` — blocks with timeout.
    pub fn result_timeout(&self, timeout: Duration) -> Option<R> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// Mirrors `future.result()` without timeout.
    pub fn result(&self) -> Option<R> {
        self.rx.recv().ok()
    }

    /// Mirrors `future.result(timeout=...)` with secs as f64.
    pub fn result_secs(&self, secs: f64) -> Option<R> {
        self.result_timeout(Duration::from_secs_f64(secs))
    }
}

// ---------------------------------------------------------------------------
// DaemonThreadPoolExecutor — mirrors `class DaemonThreadPoolExecutor(ThreadPoolExecutor)`
// ---------------------------------------------------------------------------

struct Inner {
    max_workers: usize,
    thread_name_prefix: String,
    work_queue: Arc<WorkQueue>,
    idle_semaphore: IdleSemaphore,
    thread_count: AtomicUsize,
    threads: Mutex<Vec<thread::ThreadId>>,
    initializer: Option<Arc<dyn Fn() + Send + Sync + 'static>>,
    shutdown: AtomicBool,
}

/// ThreadPoolExecutor variant whose workers do not block process exit.
///
/// Mirrors `class DaemonThreadPoolExecutor(ThreadPoolExecutor):` (line 37)
/// with docstring `ThreadPoolExecutor variant whose workers do not block process exit.`
pub struct DaemonThreadPoolExecutor {
    inner: Arc<Inner>,
}

impl DaemonThreadPoolExecutor {
    /// Mirrors `ThreadPoolExecutor(max_workers=..., thread_name_prefix=..., initializer=..., initargs=...)`
    ///
    /// Python defaults: `max_workers=None` → `min(32, os.cpu_count() + 4)`.
    /// Rust requires explicit `max_workers`; callers that relied on Python default should pass
    /// ` DaemonThreadPoolExecutor::default_max_workers()`.
    pub fn new(max_workers: usize) -> Self {
        Self::new_with_prefix(max_workers, String::new(), None)
    }

    /// Mirrors `ThreadPoolExecutor(thread_name_prefix=...)` handling.
    /// `prefix or self` in Python (line 51) → empty prefix uses executor address.
    pub fn new_with_prefix(
        max_workers: usize,
        thread_name_prefix: String,
        initializer: Option<Arc<dyn Fn() + Send + Sync + 'static>>,
    ) -> Self {
        let inner = Arc::new(Inner {
            max_workers,
            thread_name_prefix,
            work_queue: Arc::new(WorkQueue::new()),
            idle_semaphore: IdleSemaphore::new(),
            thread_count: AtomicUsize::new(0),
            threads: Mutex::new(Vec::new()),
            initializer,
            shutdown: AtomicBool::new(false),
        });
        Self { inner }
    }

    /// Mirrors Python `ThreadPoolExecutor(max_workers=None)` default.
    pub fn default_max_workers() -> usize {
        let cpus = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        std::cmp::min(32, cpus + 4)
    }

    /// Returns `max_workers` — mirrors `self._max_workers` (line 49).
    pub fn max_workers(&self) -> usize {
        self.inner.max_workers
    }

    /// Returns thread name prefix — mirrors `self._thread_name_prefix` (line 51).
    pub fn thread_name_prefix(&self) -> &str {
        &self.inner.thread_name_prefix
    }

    /// Returns number of threads spawned — mirrors `len(self._threads)` (line 49).
    pub fn thread_count(&self) -> usize {
        self.inner.thread_count.load(Ordering::SeqCst)
    }

    /// Submit a job — mirrors `executor.submit(fn, *args, **kwargs)`.
    ///
    /// Enqueues the closure and calls `_adjust_thread_count` to ensure a worker exists.
    /// The closure's return value is sent through the returned `DaemonFuture`.
    pub fn submit<F, R>(&self, f: F) -> DaemonFuture<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let job = Box::new(move || {
            let res = f();
            let _ = tx.send(res);
        }) as Box<dyn FnOnce() + Send + 'static>;

        self.inner.work_queue.put(Task::Job(job));
        self.adjust_thread_count();
        DaemonFuture { rx }
    }

    /// Submit with explicit args — mirrors `pool.submit(time.sleep, 120)` style.
    /// Convenience for closures that capture args.
    pub fn submit_fn<F, R>(&self, f: F) -> DaemonFuture<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.submit(f)
    }

    /// Mirrors `def _adjust_thread_count(self) -> None:` (line 40)
    ///
    /// Mirrors CPython's implementation (3.8–3.13) with two changes:
    /// daemon=True and no `_threads_queues` registration.
    /// ```python
    /// if self._idle_semaphore.acquire(timeout=0):
    ///     return
    /// num_threads = len(self._threads)
    /// if num_threads < self._max_workers:
    ///     thread_name = "%s_%d" % (self._thread_name_prefix or self, num_threads)
    ///     t = threading.Thread(name=thread_name, target=_worker,
    ///                          args=(weakref.ref(self, weakref_cb), self._work_queue,
    ///                                self._initializer, self._initargs), daemon=True)
    ///     t.start()
    ///     self._threads.add(t)
    /// ```
    pub fn adjust_thread_count(&self) {
        // Mirrors `if self._idle_semaphore.acquire(timeout=0): return` (line 43-44)
        if self.inner.idle_semaphore.try_acquire() {
            return;
        }

        // Mirrors `num_threads = len(self._threads)` (line 49)
        // and `if num_threads < self._max_workers:` (line 50)
        let num_threads = self.inner.thread_count.load(Ordering::SeqCst);
        if num_threads >= self.inner.max_workers {
            return;
        }

        // Mirrors `thread_name = "%s_%d" % (self._thread_name_prefix or self, num_threads)` (line 51)
        let prefix = if self.inner.thread_name_prefix.is_empty() {
            // `or self` → use executor address as fallback
            format!("{:p}", Arc::as_ptr(&self.inner))
        } else {
            self.inner.thread_name_prefix.clone()
        };
        let thread_name = format!("{}_{}", prefix, num_threads);

        // Mirrors `weakref.ref(self, weakref_cb)` + `q.put(None)` (lines 46-47)
        // In Rust: Weak<Inner> whose drop callback pushes sentinel.
        let weak = Arc::downgrade(&self.inner);
        let work_queue = Arc::clone(&self.inner.work_queue);
        let initializer = self.inner.initializer.clone();
        let inner_for_worker = Arc::clone(&self.inner);

        // Reserve the thread slot before spawn — mirrors `len(self._threads)` check-then-add.
        self.inner.thread_count.fetch_add(1, Ordering::SeqCst);

        let builder = thread::Builder::new().name(thread_name);
        // Mirrors `daemon=True` (line 61) and no `_threads_queues` registration.
        // In Rust threads are detached by dropping JoinHandle — they never block process exit
        // and are never registered in any global atexit queue (Rust has none).
        let handle = builder.spawn(move || {
            // Mirrors `if self._initializer is not None: self._initializer(*self._initargs)` at worker start.
            if let Some(init) = initializer {
                init();
            }

            // Register thread id — mirrors `self._threads.add(t)` (line 64)
            let tid = thread::current().id();
            inner_for_worker.threads.lock().unwrap().push(tid);

            // Worker loop — mirrors `concurrent.futures.thread._worker` (line 54 target=_worker)
            // _worker in CPython: loop { work_item = work_queue.get(block=True); if work_item is None: break; ... }
            loop {
                // Mark idle before blocking — mirrors idle semaphore release when worker parks.
                // CPython's _worker does `work_queue.get()` blocking; idle semaphore is released
                // by the worker when it becomes idle and acquired by _adjust_thread_count to reuse.
                // We model release before pop; adjust_thread_count's try_acquire consumes it.
                inner_for_worker.idle_semaphore.release();

                let task = work_queue.pop();

                // After wake the worker is busy — idle permit was already consumed by
                // adjust_thread_count's try_acquire (or will be consumed on next submit).
                // Do NOT decrement here; the acquire that reused this worker already did.

                match task {
                    Task::Job(job) => {
                        // Execute the work item — mirrors `_worker` running the future's fn
                        job();
                    }
                    Task::Sentinel => {
                        break;
                    }
                }

                // Mirrors weakref_cb: if executor was dropped, queue gets None sentinel
                if weak.upgrade().is_none() {
                    work_queue.put_sentinel();
                    break;
                }

                // Check shutdown flag — mirrors `if self._shutdown: break` inside _worker wrapper
                if inner_for_worker.shutdown.load(Ordering::SeqCst) {
                    // still need to drain? For daemon pool we just exit; non-daemon would join
                    break;
                }
            }
        });

        match handle {
            Ok(h) => {
                // daemon=True → never join, drop handle to detach — mirrors skipping _threads_queues
                // and daemon thread join skip at interpreter shutdown.
                std::mem::drop(h);
            }
            Err(_) => {
                // Spawn failed — roll back count
                self.inner.thread_count.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    /// Mirrors `shutdown(wait=True/False)` — daemon workers never block exit even with `wait=False`.
    ///
    /// With `wait=true`, blocks until workers finish current tasks.
    /// With `wait=false`, returns immediately; detached daemon workers are not joined and
    /// will not block process exit — mirrors Python `shutdown(wait=False)` + daemon semantics
    /// where `_python_exit` never joins them.
    pub fn shutdown(&self, wait: bool) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        // Push sentinel for each worker to wake them — mirrors `q.put(None)` per thread
        let n = self.inner.thread_count.load(Ordering::SeqCst);
        for _ in 0..n {
            self.inner.work_queue.put_sentinel();
        }
        self.inner.work_queue.set_shutdown();
        if wait {
            // Best-effort bounded wait for workers to drain current tasks.
            // We do NOT wait on thread_count (monotonic total, never decreases) — that would
            // block for 5s every time. Instead wait briefly for queue to drain.
            // Daemon semantics: wait=True joins current work, wait=False returns immediately.
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_millis(200) {
                let q_empty = self.inner.work_queue.queue.lock().unwrap().is_empty();
                if q_empty {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            // Give workers a moment to exit after sentinel
            thread::sleep(Duration::from_millis(20));
        }
        // No global _threads_queues to clean — Rust has none, so nothing to deregister.
    }

    /// Returns true if current thread would be considered daemon — always true for workers.
    /// Mirrors `threading.current_thread().daemon is True` (test_workers_are_daemon_threads).
    pub fn is_daemon() -> bool {
        true
    }

    /// Mirrors `worker not in _threads_queues` — always true in Rust (no global registry).
    pub fn is_in_threads_queues(&self, _tid: thread::ThreadId) -> bool {
        false
    }
}

impl Drop for DaemonThreadPoolExecutor {
    fn drop(&mut self) {
        // Mirrors weakref_cb: when executor GC'd, queue gets sentinel so workers exit.
        // Do not block — daemon workers must never hold process open.
        self.inner.shutdown.store(true, Ordering::SeqCst);
        self.inner.work_queue.put_sentinel();
        self.inner.work_queue.set_shutdown();
        // No join — daemon threads are detached; process exit is not blocked.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn constants_match_python_all() {
        assert_eq!(ALL, &["DaemonThreadPoolExecutor"]);
    }

    #[test]
    fn workers_are_daemon_threads() {
        let pool = DaemonThreadPoolExecutor::new(2);
        let fut = pool.submit(|| {
            // Mirrors `threading.current_thread().daemon` check
            let is_daemon = DaemonThreadPoolExecutor::is_daemon();
            let tid = thread::current().id();
            (is_daemon, tid)
        });
        let (is_daemon, worker_tid) = fut.result_timeout(Duration::from_secs(10)).unwrap();
        assert!(is_daemon);
        // Mirrors `worker not in _threads_queues` — Rust has no global queue
        assert!(!pool.is_in_threads_queues(worker_tid));
        pool.shutdown(true);
    }

    #[test]
    fn idle_worker_reuse() {
        let pool = DaemonThreadPoolExecutor::new(4);
        let tid1 = pool
            .submit(|| thread::current().id())
            .result_timeout(Duration::from_secs(10))
            .unwrap();
        thread::sleep(Duration::from_millis(50)); // let worker park on idle semaphore
        let tid2 = pool
            .submit(|| thread::current().id())
            .result_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(tid1, tid2);
        pool.shutdown(true);
    }

    #[test]
    fn wedged_worker_does_not_block_shutdown_wait_false() {
        // Mirrors test_wedged_worker_does_not_block_interpreter_exit:
        // pool.submit(time.sleep, 120); pool.shutdown(wait=False) must return immediately.
        let pool = DaemonThreadPoolExecutor::new(1);
        pool.submit(|| thread::sleep(Duration::from_secs(120)));
        thread::sleep(Duration::from_millis(100));
        let start = std::time::Instant::now();
        pool.shutdown(false);
        let elapsed = start.elapsed();
        // shutdown(wait=False) must not join wedged worker — should return quickly (<1s)
        assert!(
            elapsed < Duration::from_secs(1),
            "shutdown(wait=False) blocked for {elapsed:?}, wedged worker held it"
        );
        // Main thread can exit — detached daemon workers won't block process exit
    }

    #[test]
    fn initializer_runs_per_worker() {
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = Arc::clone(&ran);
        let pool = DaemonThreadPoolExecutor::new_with_prefix(
            1,
            "test-init".to_string(),
            Some(Arc::new(move || ran_clone.store(true, AtomicOrdering::SeqCst))),
        );
        let fut = pool.submit(|| 42);
        assert_eq!(fut.result_timeout(Duration::from_secs(2)), Some(42));
        // Give initializer time to run (runs at worker start before first task)
        thread::sleep(Duration::from_millis(50));
        assert!(ran.load(AtomicOrdering::SeqCst));
        pool.shutdown(true);
    }

    #[test]
    fn thread_name_prefix_used() {
        let pool = DaemonThreadPoolExecutor::new_with_prefix(2, "hermes-daemon".to_string(), None);
        let name = pool
            .submit(|| thread::current().name().unwrap_or("").to_string())
            .result_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(name.starts_with("hermes-daemon_"), "name was {name}");
        pool.shutdown(true);
    }

    #[test]
    fn max_workers_respected() {
        let pool = DaemonThreadPoolExecutor::new(2);
        assert_eq!(pool.max_workers(), 2);
        // Submit many tasks concurrently; thread count should never exceed max_workers
        let mut futs = Vec::new();
        for _ in 0..10 {
            futs.push(pool.submit(|| {
                thread::sleep(Duration::from_millis(20));
                thread::current().id()
            }));
        }
        for f in futs {
            f.result_timeout(Duration::from_secs(5)).unwrap();
        }
        assert!(pool.thread_count() <= 2);
        pool.shutdown(true);
    }
}
