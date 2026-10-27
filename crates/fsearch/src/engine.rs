use crate::content::{self, Content, Grep, GrepResult};
use crate::fsevents::{self, HISTORY_DONE, KERNEL_DROPPED, MUST_SCAN_SUBDIRS, USER_DROPPED};
use crate::index::Index;
use crate::live::{Applied, Live};
use crate::query::{Query, Searcher};
use crate::walk::{self, KIND_DIR, KIND_FILE, Listing, NONE, RawEnt};
use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

const COMPACT_PENDING: usize = 50_000;
const COMPACT_EVERY: Duration = Duration::from_secs(12 * 3600);
const SCAN_THREADS: usize = 8;

const MATCH_CAP: usize = 1 << 20;
const CONTENT_QUIET: Duration = Duration::from_secs(2);
const CONTENT_MAX_WAIT: Duration = Duration::from_secs(300);

const FOLLOW_EVERY: Duration = Duration::from_secs(10);

const SYNC_MARGIN: u32 = 120;

const REPLAY_QUIET: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct Options {
    pub dir: PathBuf,
    pub home: String,

    pub root: Option<PathBuf>,

    pub skip: Option<Vec<PathBuf>>,
}

pub struct Found {
    pub path: PathBuf,

    pub kind: u8,
    pub size: u64,
    pub mtime: u32,
    pub score: i32,
}

#[derive(serde::Serialize)]
pub struct Status {
    #[serde(skip)]
    pub ready: bool,
    pub entries: usize,
    pub dirs: usize,
    pub overlay: usize,
    pub removed: usize,
    pub event_id: u64,
    pub index_bytes: usize,
    pub content_docs: usize,
    pub content_segments: usize,
    pub content_bytes: usize,
    pub content_pending: usize,

    pub root_missing: bool,
    pub full_disk_access: bool,

    pub owner: bool,
}

struct Early {
    dir: Vec<u8>,
    live: Live,

    indexing: AtomicBool,
}

#[derive(Clone)]
pub struct Engine {
    s: Arc<Shared>,
}

struct Shared {
    live: RwLock<Option<Live>>,

    loading: (Mutex<bool>, Condvar),
    content: RwLock<Content>,
    home: String,

    root: Vec<u8>,
    dir: PathBuf,

    wake: Sender<Vec<fsevents::Event>>,
    save_requested: AtomicBool,
    content_tx: Sender<Resync>,
    content_rx: Mutex<Option<Receiver<Resync>>>,
    content_pending: AtomicUsize,

    dirty: content::Dirty,

    owner: AtomicBool,
    lock: std::fs::File,
    stream: Mutex<Option<fsevents::Stream>>,

    streams: std::sync::atomic::AtomicU32,
    since: AtomicU64,

    replaying: AtomicBool,

    content_seen: Mutex<Option<std::time::SystemTime>>,

    early: Mutex<Vec<Arc<Early>>>,

    content_ready: AtomicBool,

    content_early: Mutex<Vec<Vec<u8>>>,

    #[allow(clippy::type_complexity)]
    on_change: RwLock<Vec<Box<dyn Fn(&[(&[u8], bool)]) + Send + Sync>>>,

    hot: Mutex<std::collections::BTreeMap<Vec<u8>, HotScope>>,

    verified_all: AtomicU32,
}

#[derive(Clone, Copy)]
struct HotScope {
    seen: Instant,

    verified: u32,
}

const TICK_SECS: u32 = 60;

#[cfg(target_os = "linux")]
const FULL_SWEEP_SECS: u32 = 30 * 60;

#[cfg(target_os = "linux")]
const HOT_RETAIN: Duration = Duration::from_secs(2 * 3600);

const HOT_MAX: usize = 64;

fn try_lock(f: &std::fs::File) -> bool {
    unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(f), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

fn log(msg: impl AsRef<str>) {
    crate::diag::line(msg);
}

#[cfg(target_os = "macos")]
pub fn no_materialize() {
    unsafe extern "C" {
        fn setiopolicy_np(iotype: i32, scope: i32, policy: i32) -> i32;
    }

    unsafe { setiopolicy_np(3, 1, 1) };
}

#[cfg(not(target_os = "macos"))]
pub fn no_materialize() {}

#[cfg(target_os = "macos")]
type Qos = libc::qos_class_t;
#[cfg(not(target_os = "macos"))]
type Qos = u32;
#[cfg(target_os = "macos")]
const QOS_INTERACTIVE: Qos = libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE;
#[cfg(target_os = "macos")]
const QOS_UTILITY: Qos = libc::qos_class_t::QOS_CLASS_UTILITY;
#[cfg(target_os = "macos")]
const QOS_INITIATED: Qos = libc::qos_class_t::QOS_CLASS_USER_INITIATED;
#[cfg(not(target_os = "macos"))]
const QOS_INTERACTIVE: Qos = 0;
#[cfg(not(target_os = "macos"))]
const QOS_UTILITY: Qos = 0;
#[cfg(not(target_os = "macos"))]
const QOS_INITIATED: Qos = 0;

fn set_qos(#[cfg_attr(not(target_os = "macos"), allow(unused))] qos: Qos) {
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(qos, 0);
    }
}

fn search_pool() -> &'static rayon::ThreadPool {
    LAST_SEARCH.store(now_ns(), Ordering::Relaxed);
    pool()
}

static LAST_SEARCH: AtomicU64 = AtomicU64::new(0);

fn pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("fsearch-search-{i}"))
            .start_handler(|_| {
                set_qos(QOS_INTERACTIVE);
                crate::alloc::keep_on_this_thread();
            })
            .build()
            .unwrap()
    })
}

static WARM_FROM: AtomicU64 = AtomicU64::new(0);
static WARM_UNTIL: AtomicU64 = AtomicU64::new(0);

static WARM_IDLE: AtomicBool = AtomicBool::new(false);

fn now_ns() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

fn warmer() -> &'static std::thread::Thread {
    static T: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let t = std::thread::Builder::new().name("fsearch-warm".into()).spawn(|| {
            set_qos(QOS_INTERACTIVE);
            loop {
                let (from, until, now) = (WARM_FROM.load(Ordering::SeqCst), WARM_UNTIL.load(Ordering::SeqCst), now_ns());
                if now >= until {
                    WARM_IDLE.store(true, Ordering::SeqCst);
                    if now_ns() >= WARM_UNTIL.load(Ordering::SeqCst) {
                        if crate::alloc::kept() {
                            pool().spawn_broadcast(|_| crate::alloc::drain());
                        }
                        std::thread::park();
                    }
                    WARM_IDLE.store(false, Ordering::SeqCst);
                } else if now < from {
                    std::thread::park_timeout(Duration::from_nanos(from - now));
                } else {
                    std::hint::spin_loop();
                }
            }
        });
        t.expect("spawn").thread().clone()
    })
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            no_materialize();
            f()
        })
        .expect("spawn");
}

impl Engine {
    pub fn start(opts: Options) -> Result<Engine, String> {
        std::fs::create_dir_all(&opts.dir).map_err(|e| e.to_string())?;

        let mut opts = opts;
        if let Ok(c) = std::fs::canonicalize(&opts.home) {
            opts.home = c.to_string_lossy().into_owned();
        }

        let lock = std::fs::File::create(opts.dir.join("daemon.lock")).map_err(|e| e.to_string())?;
        let owner = try_lock(&lock);
        let skip: Vec<Vec<u8>> = match opts.skip {
            Some(v) => v.into_iter().map(|p| p.as_os_str().as_bytes().to_vec()).collect(),
            None if has_full_disk_access() && std::env::var_os("FSEARCH_RESTRICT").is_none() => Vec::new(),
            None => {
                log(if cfg!(target_os = "macos") {
                    "no Full Disk Access: skipping consent-gated folders (grant it to fsearch to index everything)"
                } else {
                    "skipping pseudo-filesystems (/proc, /sys, /dev, /run, ...)"
                });
                gated(&opts.home)
            }
        };
        if !skip.is_empty() {
            let _ = walk::SKIP.set(skip);
        }

        let root = {
            let mut r = opts.root.unwrap_or_else(|| PathBuf::from("/")).as_os_str().as_bytes().to_vec();
            while r.len() > 1 && r.ends_with(b"/") {
                r.pop();
            }

            match std::fs::canonicalize(std::ffi::OsStr::from_bytes(&r)) {
                Ok(c) => c.as_os_str().as_bytes().to_vec(),
                Err(_) => r,
            }
        };
        let dir = opts.dir;
        let loading = Index::load(&dir.join("index.bin"));
        let (tx, rx) = std::sync::mpsc::channel();
        let (ctx, crx) = std::sync::mpsc::channel();
        let content = if owner { Content::open(dir.join("content")) } else { Content::open_shared(dir.join("content")) };
        let content_ready = AtomicBool::new(!owner || !content.segs.is_empty());
        let shared = Arc::new(Shared {
            live: RwLock::new(None),
            loading: (Mutex::new(loading.is_some()), Condvar::new()),
            content: RwLock::new(content),
            home: opts.home,
            root,
            dir,
            wake: tx,
            save_requested: AtomicBool::new(false),
            content_tx: ctx,
            content_rx: Mutex::new(Some(crx)),
            content_pending: AtomicUsize::new(0),
            dirty: Default::default(),
            owner: AtomicBool::new(owner),
            lock,
            stream: Mutex::new(None),
            streams: Default::default(),
            since: AtomicU64::new(0),
            replaying: AtomicBool::new(true),
            content_seen: Mutex::new(None),
            early: Mutex::new(Vec::new()),
            content_ready,
            content_early: Mutex::new(Vec::new()),
            on_change: RwLock::new(Vec::new()),
            hot: Mutex::new(std::collections::BTreeMap::new()),
            verified_all: AtomicU32::new(crate::query::now_secs()),
        });
        if owner {
            start_content(&shared);
        }
        let s = shared.clone();

        spawn("fsearch-apply", move || {
            #[cfg(target_os = "linux")]
            let was_loaded = loading.is_some();
            let (live, replay) = match loading {
                Some(b) => {
                    log(format!("loaded {} entries, replaying events since {}", b.n, b.event_id));
                    let from = b.event_id;
                    (Live::lazy(b).rooted(&s.root), Some(from))
                }
                None if owner => {
                    let since = fsevents::current_id();
                    s.watch(since);
                    (Live::lazy(full_build(&s, since)).rooted(&s.root), None)
                }
                None => {
                    let b = wait_for_index(&s.dir);
                    s.watch(b.event_id);
                    (Live::lazy(b).rooted(&s.root), None)
                }
            };
            *s.live.write().unwrap() = Some(live);
            s.early.lock().unwrap().clear();
            *s.loading.0.lock().unwrap() = false;
            s.loading.1.notify_all();
            if let Some(from) = replay {
                s.watch(if owner && from == 0 { fsevents::current_id() } else { from });
            }
            if let Some(l) = s.live.read().unwrap().as_ref() {
                l.base.prefault();
            }
            if owner {
                rescan_unskipped(&s);
            }

            #[cfg(target_os = "linux")]
            if was_loaded {
                relist_changed(&s, "restart without an event log", 0);
            }
            apply_loop(&s, rx);
        });
        Ok(Engine { s: shared })
    }

    pub fn home(&self) -> &str {
        &self.s.home
    }

    pub fn keep_warm(&self, d: Duration) {
        let w = warmer();
        let now = now_ns();

        WARM_FROM.store(now + 2_000_000, Ordering::SeqCst);
        WARM_UNTIL.store(now + d.as_nanos() as u64, Ordering::SeqCst);
        if WARM_IDLE.load(Ordering::SeqCst) {
            w.unpark();
        }
    }

    fn saw_scope(&self, scope: Option<&[u8]>) {
        let Some(scope) = scope.filter(|s| s.len() > 1) else { return };
        {
            let mut h = self.s.hot.lock().unwrap();
            if let Some(r) = h.get_mut(scope) {
                r.seen = Instant::now();
            } else {
                if h.len() >= HOT_MAX
                    && let Some(oldest) = h.iter().min_by_key(|(_, s)| s.seen).map(|(p, _)| p.clone())
                {
                    h.remove(&oldest);
                }
                h.insert(scope.to_vec(), HotScope { seen: Instant::now(), verified: 0 });
            }
        }
        let now = crate::query::now_secs();
        let bound = now.saturating_sub(TICK_SECS);
        let stale = {
            let h = self.s.hot.lock().unwrap();
            let cover = h.iter().filter(|(p, _)| under(scope, p)).map(|(_, s)| s.verified).max().unwrap_or(0);
            cover.max(self.s.verified_all.load(Ordering::Relaxed))
        };
        if stale >= bound {
            return;
        }
        let from = stale.saturating_sub(SYNC_MARGIN);
        let mut dirs = {
            let g = self.s.live.read().unwrap();
            let Some(live) = g.as_ref() else { return };
            live.changed_dirs_in(from, &[scope.to_vec()])
        };
        dirs.extend(self.s.content.read().unwrap().changed_dirs_in(from, &[scope.to_vec()]));
        dirs.sort();
        dirs.dedup();
        let t = Instant::now();
        let n = dirs.len();
        apply_dirs(&self.s, dirs);
        if let Some(r) = self.s.hot.lock().unwrap().get_mut(scope) {
            r.verified = now;
        }
        if n > 0 {
            log(format!("cold scope {}: relisted {n} folders in {t:.2?}", String::from_utf8_lossy(scope)));
        }
    }

    pub fn search(&self, q: &Query) -> Result<Vec<Found>, String> {
        self.saw_scope(q.scope.as_deref());
        WARM_UNTIL.store(0, Ordering::Relaxed);
        let (found, early) = self.s.with_names(q.scope.as_deref(), |live| {
            let mut p = Vec::new();
            search_pool()
                .install(|| Searcher { live }.search(q))
                .into_iter()
                .map(|h| {
                    let (kind, size, mtime) = match &h.over {
                        Some(path) => {
                            let o = live.over[path];
                            p = path.clone();
                            (o.kind, o.size, o.mtime)
                        }
                        None => {
                            let i = h.idx as usize;
                            live.base.path(i, &mut p);
                            (live.base.kind(i), live.base.size_of(i), live.base.mtime(i))
                        }
                    };
                    Found { path: PathBuf::from(std::ffi::OsStr::from_bytes(&p)), kind, size, mtime, score: h.score }
                })
                .collect()
        })?;
        self.s.index_first(early);
        Ok(found)
    }

    pub fn covers(&self, path: &[u8]) -> bool {
        let g = self.s.live();
        let Some(live) = g.as_ref() else { return false };
        live.base.lookup(path).and_then(|e| live.base.dir_of(e)).is_some()
    }

    pub fn grep(&self, q: &Query, g: &Grep) -> Result<(GrepResult, bool), String> {
        self.saw_scope(q.scope.as_deref());
        WARM_UNTIL.store(0, Ordering::Relaxed);
        let home = self.s.home.as_bytes();
        let indexed = q.scope.as_ref().is_none_or(|s| content::in_scope(s, home));
        let built = indexed && self.s.content_holds(q.scope.as_deref());
        if built {
            let fresh = self.s.dirty.files(q);
            let c = self.s.content.read().unwrap();

            if q.scope.as_ref().is_none_or(|s| c.holds(s)) {
                return Ok((c.search_with(g, q, &fresh), true));
            }
        }

        let scope = q.scope.as_deref();
        if indexed && !built {
            let (docs, early) = self.s.with_names(scope, |live| content::wanted(live, home, scope.unwrap_or(home), true))?;
            if !docs.is_empty() {
                let hold = walk::Hold::new();
                let r = content::grep_unindexed(g, docs, q);
                drop(hold);
                self.s.index_first(early);
                return Ok((r, false));
            }
            self.s.index_first(early);
        }
        let (paths, _) = self.s.with_names(scope, |live| search_pool().install(|| content::scan_paths(live, q.clone_for_scan())))?;
        let _hold = walk::Hold::new();
        Ok((content::verify(g, &paths, q.limit), false))
    }

    pub fn unheld_files(&self, scope: &[u8]) -> Option<Vec<Vec<u8>>> {
        self.saw_scope(Some(scope));
        if !self.s.content_holds(Some(scope)) {
            return None;
        }
        let home = self.s.home.as_bytes();
        let (out, early) = self
            .s
            .with_names(Some(scope), |live| {
                let mut want = content::wanted(live, home, scope, true);
                want.sort();
                let c = self.s.content.read().unwrap();
                let mut out: Vec<Vec<u8>> = c.unheld(scope, true, &want).into_iter().map(|k| want.path(k).to_vec()).collect();
                out.extend(c.not_text(scope, true));
                drop(c);
                out.extend(files_under(live, scope).into_iter().filter(|p| want.find(p).is_none()));
                out.sort_unstable();
                out.dedup();
                out
            })
            .ok()?;
        self.s.index_first(early);
        Some(out)
    }

    pub fn held_matches(&self, scope: &[u8], pats: &[String], ignore_case: bool) -> Option<Vec<Vec<u8>>> {
        self.saw_scope(Some(scope));
        if !self.s.content_holds(Some(scope)) {
            return None;
        }
        let case = if ignore_case { content::Case::Insensitive } else { content::Case::Sensitive };
        let mut g = Grep::any_of(pats, case).ok()?;
        g.max_per_file = 1;
        g.budget = None;
        let q = Query { scope: Some(scope.to_vec()), limit: MATCH_CAP, size: (0, u64::MAX), mtime: (0, u32::MAX), ..Default::default() };
        let (res, _) = self.grep(&q, &g).ok()?;
        if !res.complete || res.next.is_some() || res.files.len() >= MATCH_CAP {
            return None;
        }
        Some(res.files.into_iter().map(|f| f.path).collect())
    }

    pub fn status(&self) -> Status {
        let l = self.s.live.read().unwrap();
        let c = self.s.content.read().unwrap();
        Status {
            ready: l.is_some(),
            entries: l.as_ref().map_or(0, |l| l.base.n),
            dirs: l.as_ref().map_or(0, |l| l.base.d),
            overlay: l.as_ref().map_or(0, |l| l.over.len()),
            removed: l.as_ref().map_or(0, |l| l.dead_count),
            event_id: l.as_ref().map_or(0, |l| l.event_id),
            index_bytes: l.as_ref().map_or(0, |l| l.base.bytes()),
            content_docs: c.docs(),
            content_segments: c.segs.len(),
            content_bytes: c.bytes(),

            content_pending: self.s.content_pending.load(Ordering::Relaxed) + usize::from(!self.s.content_ready.load(Ordering::Acquire)),
            root_missing: l.as_ref().is_some_and(|l| l.root_missing.load(Ordering::Relaxed)),
            full_disk_access: walk::SKIP.get().is_none(),
            owner: self.s.owner(),
        }
    }

    pub fn indexing(&self) -> String {
        indexing()
    }

    pub fn save(&self) {
        self.s.save_requested.store(true, Ordering::Relaxed);
        let _ = self.s.wake.send(Vec::new());
    }

    pub fn on_change(&self, f: impl Fn(&[(&[u8], bool)]) + Send + Sync + 'static) {
        self.s.on_change.write().unwrap().push(Box::new(f));
    }
}

type Resync = (Vec<Vec<u8>>, Vec<Vec<u8>>);

fn indexing() -> String {
    let n = walk::LISTED.load(Ordering::Relaxed);
    format!("indexing: {:.1}M entries so far (the first run lists the whole disk, ~25s)", n as f64 / 1e6)
}

impl Shared {
    fn live(&self) -> RwLockReadGuard<'_, Option<Live>> {
        let (loading, cv) = &self.loading;
        drop(cv.wait_while(loading.lock().unwrap(), |l| *l).unwrap());
        self.live.read().unwrap()
    }

    fn owner(&self) -> bool {
        self.owner.load(Ordering::Relaxed)
    }

    fn with_names<R>(&self, scope: Option<&[u8]>, f: impl FnOnce(&Live) -> R) -> Result<(R, Option<Arc<Early>>), String> {
        loop {
            if let Some(live) = self.live().as_ref() {
                return Ok((f(live), None));
            }
            let Some(dir) = scope.filter(|s| s.len() > 1) else { return Err(indexing()) };
            if let Some(early) = self.early(dir) {
                return Ok((f(&early.live), Some(early)));
            }
        }
    }

    fn early(&self, dir: &[u8]) -> Option<Arc<Early>> {
        let mut early = self.early.lock().unwrap();
        if self.live.read().unwrap().is_some() {
            return None;
        }
        if let Some(e) = early.iter().find(|e| under(dir, &e.dir)) {
            return Some(e.clone());
        }
        let t = Instant::now();
        let hold = walk::Hold::new();
        let ls = folder_listings(dir);
        drop(hold);
        let live = Live::new(Index::build(ls, 0, 0, self.home.as_bytes()));
        log(format!("listed {} first: {} entries in {:.2?}", String::from_utf8_lossy(dir), live.base.n, t.elapsed()));
        let e = Arc::new(Early { dir: dir.to_vec(), live, indexing: AtomicBool::new(false) });
        early.push(e.clone());
        Some(e)
    }

    fn index_first(&self, early: Option<Arc<Early>>) {
        if let Some(e) = early
            && self.owner()
            && content::in_scope(&e.dir, self.home.as_bytes())
            && !e.indexing.swap(true, Ordering::Relaxed)
        {
            let _ = self.content_tx.send((Vec::new(), vec![e.dir.clone()]));
        }
    }

    fn content_holds(&self, scope: Option<&[u8]>) -> bool {
        self.content_ready.load(Ordering::Acquire) || scope.is_some_and(|s| self.content_early.lock().unwrap().iter().any(|f| under(s, f)))
    }

    fn watch(&self, since: u64) {
        self.replaying.store(true, Ordering::Relaxed);
        self.since.store(since, Ordering::Relaxed);
        let n = self.streams.fetch_add(1, Ordering::Relaxed) + 1;

        let new = fsevents::watch(since, 0.01, self.wake.clone(), n);
        *self.stream.lock().unwrap() = Some(new);
    }

    fn follow(&self) {
        let path = self.dir.join("index.bin");
        let saved = Index::saved_event_id(&path).unwrap_or(0);
        let ours = self.live.read().unwrap().as_ref().map_or(0, |l| l.base.event_id);
        if saved > ours
            && let Some(base) = Index::load(&path)
        {
            log(format!("following the owner's save: {} entries, replaying since {}", base.n, base.event_id));
            self.watch(base.event_id);
            *self.live.write().unwrap() = Some(Live::new(base).rooted(&self.root));
        }
        let cdir = self.dir.join("content");
        let changed = std::fs::metadata(&cdir).and_then(|m| m.modified()).ok();
        let mut seen = self.content_seen.lock().unwrap();
        if changed != *seen {
            *seen = changed;
            *self.content.write().unwrap() = Content::open_shared(cdir);
        }
    }
}

fn start_content(s: &Arc<Shared>) {
    let Some(rx) = s.content_rx.lock().unwrap().take() else {
        return;
    };
    let s = s.clone();
    spawn("fsearch-content", move || content_loop(&s, rx));
}

fn try_upgrade(s: &Arc<Shared>) -> bool {
    if s.owner() {
        return true;
    }
    if !try_lock(&s.lock) {
        return false;
    }
    s.owner.store(true, Ordering::Relaxed);
    log("took over the index from a previous owner");
    *s.content.write().unwrap() = Content::open(s.dir.join("content"));
    rescan_unskipped(s);
    start_content(s);
    true
}

const SKIPPED: &str = "skipped";

fn note_skipped(dir: &Path) {
    let denied = walk::DENIED.lock().unwrap();

    let mut all: Vec<&Vec<u8>> = walk::SKIP.get().into_iter().flatten().chain(denied.iter()).collect();
    all.sort();
    all.dedup();
    let mut out = Vec::new();
    for p in all {
        out.extend_from_slice(p);
        out.push(b'\n');
    }
    if let Err(e) = std::fs::write(dir.join(SKIPPED), out) {
        log(format!("save failed: {e}"));
    }
}

fn rescan_unskipped(shared: &Shared) {
    let was: Vec<Vec<u8>> = match std::fs::read(shared.dir.join(SKIPPED)) {
        Ok(b) => b.split(|&c| c == b'\n').filter(|l| !l.is_empty()).map(<[u8]>::to_vec).collect(),
        Err(_) => gated(&shared.home),
    };
    let mut now = Vec::new();
    for path in was {
        if walk::blocked(&path) {
            continue;
        }
        let readable = std::fs::read_dir(std::ffi::OsStr::from_bytes(&path)).map_or_else(|e| e.raw_os_error() != Some(libc::EPERM), |_| true);
        if readable {
            now.push(fsevents::Event { path, flags: MUST_SCAN_SUBDIRS, id: 0, stream: 0 });
        } else {
            walk::DENIED.lock().unwrap().push(path);
        }
    }
    if now.is_empty() {
        return;
    }
    log(format!("rescanning {} folders the saved index lacked", now.len()));
    let _ = shared.wake.send(now);

    shared.save_requested.store(true, Ordering::Relaxed);
}

fn files_under(live: &Live, scope: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut p = Vec::new();
    if let Some(top) = live.base.lookup(scope).and_then(|e| live.base.dir_of(e)) {
        let mut stack = vec![top];
        while let Some(k) = stack.pop() {
            for i in live.base.children(k) {
                if live.is_dead(i as u32) {
                    continue;
                }
                match live.base.kind(i) & 3 {
                    KIND_DIR => stack.extend(live.base.dir_of(i as u32)),
                    KIND_FILE => {
                        live.base.path(i, &mut p);
                        out.push(p.clone());
                    }
                    _ => {}
                }
            }
        }
    }
    out.extend(
        live.over.range(scope.to_vec()..).take_while(|(p, _)| under(p, scope)).filter(|(_, o)| o.kind & 3 == KIND_FILE).map(|(p, _)| p.clone()),
    );
    out
}

pub(crate) fn under(path: &[u8], dir: &[u8]) -> bool {
    path.starts_with(dir) && (path.len() == dir.len() || path[dir.len()] == b'/')
}

const EARLY_THREADS: usize = 16;

fn folder_listings(dir: &[u8]) -> Vec<Listing> {
    let comps: Vec<&[u8]> = dir.split(|&b| b == b'/').filter(|c| !c.is_empty()).collect();
    let (mut out, mut path) = (Vec::new(), b"/".to_vec());
    for (i, &c) in comps.iter().enumerate() {
        let found = walk::list_one(&path).and_then(|l| l.ents.iter().copied().find(|e| &l.names[e.name_off as usize..][..e.name_len as usize] == c));
        let Some(e) = found else { return out };
        path = crate::live::join(&path, c);
        let descend = e.kind & 3 == KIND_DIR && e.kind & walk::FLAG_MOUNT == 0 && !walk::blocked(&path);
        let child = if descend { i as u32 + 1 } else { NONE };
        out.push(Listing { id: i as u32, names: c.to_vec(), ents: vec![RawEnt { name_off: 0, child, ..e }] });
        if !descend {
            return out;
        }
    }
    let k = comps.len() as u32;
    for mut l in walk::scan(dir, EARLY_THREADS) {
        l.id += k;
        for e in l.ents.iter_mut().filter(|e| e.child != NONE) {
            e.child += k;
        }
        out.push(l);
    }
    out
}

fn wait_for_index(dir: &Path) -> Index {
    loop {
        if let Some(b) = Index::load(&dir.join("index.bin")) {
            return b;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(target_os = "macos")]
#[rustfmt::skip]
const GATED_IN_HOME: &[&str] = &[
    "Desktop", "Documents", "Downloads", "Library/Mobile Documents", "Library/Containers", "Library/Group Containers",
    "Library/CloudStorage", "Pictures/Photos Library.photoslibrary", "Music/Music", "Movies/TV",
];

#[cfg(target_os = "macos")]
pub fn gated(home: &str) -> Vec<Vec<u8>> {
    GATED_IN_HOME.iter().map(|d| format!("{home}/{d}").into_bytes()).chain([b"/Volumes".to_vec()]).collect()
}

#[cfg(target_os = "linux")]
pub fn gated(_home: &str) -> Vec<Vec<u8>> {
    ["/proc", "/sys", "/dev", "/run", "/boot/efi", "/var/lib/docker", "/var/lib/containers", "/snap"].iter().map(|s| s.as_bytes().to_vec()).collect()
}

#[cfg(target_os = "macos")]
pub fn has_full_disk_access() -> bool {
    std::fs::File::open("/Library/Application Support/com.apple.TCC/TCC.db").is_ok()
}

#[cfg(not(target_os = "macos"))]
pub fn has_full_disk_access() -> bool {
    false
}

fn content_pool(threads: usize, qos: Qos) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .start_handler(move |_| {
            set_qos(qos);
            no_materialize();
        })
        .build()
        .unwrap()
}

fn content_loop(shared: &Shared, rx: Receiver<Resync>) {
    set_qos(QOS_UTILITY);
    let pool = content_pool(4, QOS_UTILITY);
    let home = shared.home.as_bytes().to_vec();

    while shared.live.read().unwrap().is_none() {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok((_, dirs)) => {
                for dir in dirs {
                    let Some(e) = shared.early.lock().unwrap().iter().find(|e| e.dir == dir).cloned() else { continue };
                    sync_wants(shared, &pool, content::wants(&e.live, &home, &[], std::slice::from_ref(&dir)));
                    shared.content_early.lock().unwrap().push(dir);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }

    sync(shared, &pool, &home, &[], std::slice::from_ref(&home));
    shared.content_ready.store(true, Ordering::Release);

    let mut pending: HashMap<(Vec<u8>, bool), (Instant, Instant)> = HashMap::new();
    loop {
        let wait = if pending.is_empty() { Duration::from_secs(3600) } else { Duration::from_millis(250) };
        match rx.recv_timeout(wait) {
            Ok(first) => {
                let now = Instant::now();
                let mut changed = Vec::new();
                for (d, t) in std::iter::once(first).chain(rx.try_iter()) {
                    for key in d.into_iter().map(|p| (p, false)).chain(t.into_iter().map(|p| (p, true))) {
                        if content::in_scope(&key.0, &home) || (key.1 && home.starts_with(&key.0)) {
                            pending.entry(key.clone()).and_modify(|e| e.1 = now).or_insert((now, now));
                            changed.push(key);
                        }
                    }
                }

                changed.sort();
                changed.dedup();
                let wants = shared.live.read().unwrap().as_ref().map(|l| content::wanted_now(l, &home, &changed));
                if let Some(w) = wants {
                    shared.dirty.mark(&shared.content.read().unwrap(), w);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let ripe: Vec<(Vec<u8>, bool)> = pending
            .iter()
            .filter(|(_, (first, last))| last.elapsed() >= CONTENT_QUIET || first.elapsed() >= CONTENT_MAX_WAIT)
            .map(|(k, _)| k.clone())
            .collect();
        if ripe.is_empty() {
            continue;
        }
        let (mut dirs, mut trees) = (Vec::<Vec<u8>>::new(), Vec::<Vec<u8>>::new());
        for k in &ripe {
            pending.remove(k);
            if k.1 { trees.push(k.0.clone()) } else { dirs.push(k.0.clone()) }
        }
        sync(shared, &pool, &home, &dirs, &trees);
        shared.dirty.clear(&ripe);
    }
}

fn sync(shared: &Shared, pool: &rayon::ThreadPool, home: &[u8], dirs: &[Vec<u8>], trees: &[Vec<u8>]) {
    let wants = {
        let g = shared.live.read().unwrap();
        let Some(live) = g.as_ref() else { return };
        content::wants(live, home, dirs, trees)
    };
    sync_wants(shared, pool, wants);
}

fn sync_wants(shared: &Shared, pool: &rayon::ThreadPool, wants: Vec<(Vec<u8>, bool, content::Docs)>) {
    let t = Instant::now();

    let first = !shared.content_ready.load(Ordering::Acquire);
    let todo = shared.content.write().unwrap().diff(wants);
    if todo.is_empty() {
        return;
    }
    let n = todo.len();
    shared.content_pending.store(n, Ordering::Relaxed);

    let fast;
    let pool = if first {
        let threads = std::thread::available_parallelism().map_or(8, |n| n.get()).min(8);
        fast = content_pool(threads, QOS_INITIATED);
        &fast
    } else {
        pool
    };
    build_batches(shared, pool, &todo);
    drop(todo);

    loop {
        let plan = shared.content.read().unwrap().merge_plan();
        let Some(ids) = plan else { break };
        let (dir, id) = {
            let mut c = shared.content.write().unwrap();
            (c.dir.clone(), c.alloc_id())
        };
        let merged = {
            let c = shared.content.read().unwrap();
            pool.install(|| content::merge(&dir, id, &c.segments(&ids)))
        };
        match merged {
            Some(seg) => shared.content.write().unwrap().replace(&ids, seg),
            None => break,
        }
    }
    if n > 100 {
        log(format!("content: indexed {n} files in {:.2?}", t.elapsed()));
    }
    release_memory();
}

fn build_batches(shared: &Shared, pool: &rayon::ThreadPool, todo: &content::Docs) {
    let dir = shared.content.read().unwrap().dir.clone();
    for range in todo.batches() {
        walk::yield_to_searches();
        let id = shared.content.write().unwrap().alloc_id();
        if let Some(seg) = pool.install(|| content::build_segment(&dir, id, todo, range.clone())) {
            shared.content.write().unwrap().push(seg);
        }
        shared.content_pending.fetch_sub(range.len(), Ordering::Relaxed);
    }
}

fn scan_rooted(root: &[u8], threads: usize) -> Vec<walk::Listing> {
    let mut ls = if root == b"/" { walk::scan_disk(threads) } else { walk::scan(root, threads) };
    let comps: Vec<&[u8]> = root.split(|&b| b == b'/').filter(|c| !c.is_empty()).collect();
    if comps.is_empty() {
        return ls;
    }

    let mut next = ls.iter().map(|l| l.id).max().unwrap_or(0) + 1;
    let real_root = next;
    next += 1;
    for l in &mut ls {
        if l.id == 0 {
            l.id = real_root;
        }
    }

    let mut ids = vec![0u32; comps.len() + 1];
    ids[comps.len()] = real_root;
    for d in ids.iter_mut().take(comps.len()).skip(1) {
        *d = next;
        next += 1;
    }
    let mut parent = vec![b'/'];
    for (d, comp) in comps.iter().enumerate() {
        let mtime = crate::live::lstat(&parent).map_or(0, |o| o.mtime);
        let kind = walk::KIND_DIR | (u8::from(comp[0] == b'.') * walk::FLAG_HIDDEN);
        ls.push(walk::Listing {
            id: ids[d],
            names: comp.to_vec(),
            ents: vec![walk::RawEnt { name_off: 0, name_len: comp.len() as u16, kind, size: 0, mtime, child: ids[d + 1] }],
        });
        parent.extend_from_slice(comp);
        parent.push(b'/');
    }
    ls
}

fn full_build(shared: &Shared, event_id: u64) -> Index {
    let t = Instant::now();
    let started = crate::query::now_secs();
    let ls = scan_rooted(&shared.root, SCAN_THREADS);
    let idx = Index::build(ls, event_id, started, shared.home.as_bytes());
    let path = shared.dir.join("index.bin");
    if let Err(e) = idx.save(&path) {
        log(format!("save failed: {e}"));
    }
    note_skipped(&shared.dir);
    log(format!("indexed {} entries in {:.2?}", idx.n, t.elapsed()));
    release_memory();

    Index::load(&path).unwrap_or(idx)
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
}

fn release_memory() {
    #[cfg(target_os = "macos")]
    unsafe {
        malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
    #[cfg(all(target_os = "linux", not(target_env = "musl")))]
    unsafe {
        libc::malloc_trim(0);
    }
}

fn compact(shared: &Shared) {
    let t = Instant::now();
    let (ls, eid, synced) = {
        let g = shared.live.read().unwrap();
        let live = g.as_ref().unwrap();
        (live.to_listings(), live.event_id, live.synced_at)
    };

    let eid = if cfg!(target_os = "linux") { fsevents::current_id() } else { eid };
    let idx = Index::build(ls, eid, synced, shared.home.as_bytes());
    let path = shared.dir.join("index.bin");
    if let Err(e) = idx.save(&path) {
        log(format!("save failed: {e}"));
    }
    note_skipped(&shared.dir);
    let idx = Index::load(&path).unwrap_or(idx);
    let n = idx.n;
    *shared.live.write().unwrap() = Some(Live::new(idx).rooted(&shared.root));
    release_memory();
    log(format!("compacted to {n} entries in {:.2?}", t.elapsed()));
}

fn sweep_from(shared: &Shared) -> u32 {
    let g = shared.live.read().unwrap();
    let live = g.as_ref().unwrap();
    let missing = crate::live::lstat(&live.root).is_none();
    let was = live.root_missing.fetch_or(missing, Ordering::Relaxed);
    if missing || was { 0 } else { live.synced_at.saturating_sub(SYNC_MARGIN) }
}

fn apply_dirs(shared: &Shared, dirs: Vec<Vec<u8>>) {
    for d in &dirs {
        let f = shared.live.read().unwrap().as_ref().unwrap().fetch(d, false);
        shared.live.write().unwrap().as_mut().unwrap().apply(f);
    }
    let trees = {
        let mut g = shared.live.write().unwrap();
        let live = g.as_mut().unwrap();
        live.synced_at = crate::query::now_secs();
        std::mem::take(&mut live.trees)
    };
    let _ = shared.content_tx.send((dirs, trees));
    shared.on_change.read().unwrap().iter().for_each(|f| f(&[(&b"/"[..], true)]));
}

fn relist_changed(shared: &Arc<Shared>, why: &str, flags: u32) {
    let t = Instant::now();
    let from = sweep_from(shared);
    let mut dirs = {
        let g = shared.live.read().unwrap();
        g.as_ref().unwrap().changed_dirs(from)
    };
    dirs.extend(shared.content.read().unwrap().changed_dirs(from));
    dirs.sort();
    dirs.dedup();
    let stat_time = t.elapsed();
    let n = dirs.len();
    apply_dirs(shared, dirs);
    log(format!(
        "FSEvents lost track of / ({why}, flags {flags:#x}): relisted {n} folders changed since {from} in {:.2?} ({stat_time:.2?} checking)",
        t.elapsed()
    ));
}

#[cfg(target_os = "linux")]
fn tick_reconcile(shared: &Arc<Shared>) {
    let t = Instant::now();
    let now = crate::query::now_secs();
    let hot: Vec<Vec<u8>> = {
        let mut h = shared.hot.lock().unwrap();
        h.retain(|_, s| s.seen.elapsed() < HOT_RETAIN);
        h.keys().cloned().collect()
    };
    let full_due = now >= shared.verified_all.load(Ordering::Relaxed).saturating_add(FULL_SWEEP_SECS);
    if hot.is_empty() && !full_due {
        return;
    }
    let from = sweep_from(shared);
    let mut dirs = Vec::new();
    if !hot.is_empty() {
        dirs.extend({
            let g = shared.live.read().unwrap();
            g.as_ref().unwrap().changed_dirs_in(from, &hot)
        });
        dirs.extend(shared.content.read().unwrap().changed_dirs_in(from, &hot));
    }
    if full_due {
        dirs.extend({
            let g = shared.live.read().unwrap();
            g.as_ref().unwrap().changed_dirs(from)
        });
        dirs.extend(shared.content.read().unwrap().changed_dirs(from));
    }
    dirs.sort();
    dirs.dedup();
    let stat_time = t.elapsed();
    let n = dirs.len();
    apply_dirs(shared, dirs);
    if full_due {
        shared.verified_all.store(now, Ordering::Relaxed);
    }
    {
        let mut h = shared.hot.lock().unwrap();
        for k in &hot {
            if let Some(s) = h.get_mut(k) {
                s.verified = now;
            }
        }
    }
    if n > 0 || full_due {
        let what = if full_due { "full reconcile" } else { "reconcile" };
        log(format!("index {what}: relisted {n} folders changed since {from} in {:.2?} ({stat_time:.2?} checking)", t.elapsed()));
    }
}

const RELIST_SHARE: u32 = 50;
const RELIST_AHEAD: Duration = Duration::from_millis(2500);

#[derive(Default)]
struct Pace {
    paid: HashMap<Vec<u8>, Instant>,

    held: HashMap<Vec<u8>, (bool, u64)>,

    kept: usize,
}

impl Pace {
    fn free(&self, dir: &[u8], now: Instant) -> bool {
        self.paid.get(dir).is_none_or(|&t| t <= now + RELIST_AHEAD)
    }

    fn sort(&mut self, dirs: &mut HashMap<Vec<u8>, (bool, u64)>, now: Instant) {
        let early: Vec<Vec<u8>> = dirs.keys().filter(|d| !self.free(d, now)).cloned().collect();
        for d in early {
            let (r, id) = dirs.remove(&d).unwrap();
            let h = self.held.entry(d).or_insert((r, id));
            *h = (h.0 | r, h.1.min(id));
        }
        let due: Vec<Vec<u8>> = self.held.keys().filter(|d| self.free(d, now)).cloned().collect();
        for d in due {
            let (r, id) = self.held.remove(&d).unwrap();
            let e = dirs.entry(d).or_insert((r, id));
            *e = (e.0 | r, e.1.min(id));
        }
    }

    fn listed(&mut self, dir: &[u8], took: Duration, now: Instant) {
        if self.paid.len() > 2 * self.kept.max(2048) {
            self.paid.retain(|_, t| *t > now);
            self.kept = self.paid.len();
        }

        let from = self.paid.get(dir).map_or(now, |&t| t.max(now));
        self.paid.insert(dir.to_vec(), (from + took * RELIST_SHARE).min(now + RELIST_AHEAD + Duration::from_secs(1)));
    }

    fn wait(&self, now: Instant) -> Option<Duration> {
        let free_at = |d: &Vec<u8>| self.paid.get(d).map_or(now, |&t| t.checked_sub(RELIST_AHEAD).unwrap_or(now));
        self.held.keys().map(free_at).min().map(|t| t.saturating_duration_since(now))
    }

    fn floor(&self) -> u64 {
        self.held.values().map(|h| h.1).min().unwrap_or(u64::MAX)
    }
}

fn drop_stale_events(events: &mut Vec<fsevents::Event>, current: u32) -> usize {
    let was = events.len();
    events.retain(|e| e.stream == 0 || e.stream == current);
    was - events.len()
}

fn apply_loop(shared: &Arc<Shared>, rx: Receiver<Vec<fsevents::Event>>) {
    let mut last_save = Instant::now();
    let mut last_follow = Instant::now();
    let ours = shared.dir.as_os_str().as_bytes();

    let mut gap: Option<(u64, Instant)> = None;

    let mut replays = 0;
    let mut pace = Pace::default();
    loop {
        let wait = if gap.is_some() { Duration::from_millis(250) } else { Duration::from_secs(60) };
        let wait = pace.wait(Instant::now()).map_or(wait, |w| w.min(wait));
        let mut events = match rx.recv_timeout(wait) {
            Ok(b) => b,
            Err(RecvTimeoutError::Timeout) => {
                #[cfg(target_os = "linux")]
                if shared.owner() && shared.live.read().unwrap().is_some() {
                    tick_reconcile(shared);
                }
                Vec::new()
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        events.extend(rx.try_iter().flatten());

        let stale = drop_stale_events(&mut events, shared.streams.load(Ordering::Relaxed));
        if stale > 0 {
            log(format!("dropped {stale} events from a replaced stream"));
        }

        let owner_wrote = events.iter().any(|e| e.path.starts_with(ours));
        let (mut rebuild, mut root_flags, had_events) = (false, 0, !events.is_empty());
        let (mut dirs, mut max_id): (HashMap<Vec<u8>, (bool, u64)>, u64) = (HashMap::new(), 0);
        if had_events {
            let mut last_good = shared.live.read().unwrap().as_ref().map_or(0, |l| l.event_id);
            let current = shared.streams.load(Ordering::Relaxed);
            for e in events {
                if e.path == b"/" && e.flags & MUST_SCAN_SUBDIRS != 0 {
                    if e.flags & (USER_DROPPED | KERNEL_DROPPED) == USER_DROPPED {
                        if e.stream == current {
                            let from = if shared.replaying.load(Ordering::Relaxed) { shared.since.load(Ordering::Relaxed) } else { last_good };
                            gap = Some((gap.map_or(from, |(g, _)| g.min(from)), Instant::now()));
                        }
                        continue;
                    }
                    root_flags |= e.flags;
                }
                max_id = max_id.max(e.id);
                if gap.is_none() {
                    last_good = last_good.max(e.id);
                }
                if e.flags & HISTORY_DONE != 0 {
                    log("replay done");
                    shared.replaying.store(false, Ordering::Relaxed);
                    if gap.is_none() {
                        replays = 0;
                    }
                    continue;
                }
                let d = dirs.entry(crate::live::normalize(&e.path)).or_insert((false, e.id));
                *d = (d.0 | (e.flags & MUST_SCAN_SUBDIRS != 0), d.1.min(e.id));
            }
        }
        let now = Instant::now();
        pace.sort(&mut dirs, now);
        if !dirs.is_empty() {
            let hooks = shared.on_change.read().unwrap();
            if !hooks.is_empty() {
                let changed: Vec<(&[u8], bool)> = dirs.iter().map(|(p, &(r, _))| (p.as_slice(), r)).collect();
                hooks.iter().for_each(|f| f(&changed));
            }
            drop(hooks);
            let mut trees = Vec::new();

            let fetched: Vec<_> = {
                let g = shared.live.read().unwrap();
                let live = g.as_ref().unwrap();
                dirs.iter()
                    .map(|(p, &(recursive, _))| {
                        let t = Instant::now();
                        let f = live.fetch(p, recursive);
                        pace.listed(p, t.elapsed(), now);
                        f
                    })
                    .collect()
            };
            {
                let mut g = shared.live.write().unwrap();
                let live = g.as_mut().unwrap();
                for f in fetched {
                    if let Applied::Rebuild = live.apply(f) {
                        rebuild = true;
                    }
                }

                live.event_id = match gap {
                    Some((from, _)) => live.event_id.min(from),
                    None => live.event_id.max(max_id.min(pace.floor().saturating_sub(1))),
                };
                trees.append(&mut live.trees);
            }
            let (rec, flat): (Vec<_>, Vec<_>) = dirs.into_iter().partition(|(_, (r, _))| *r);
            trees.extend(rec.into_iter().map(|(p, _)| p));
            let _ = shared.content_tx.send((flat.into_iter().map(|(p, _)| p).collect(), trees));
        }
        let replay = gap.filter(|(_, at)| at.elapsed() >= REPLAY_QUIET).map(|(from, _)| from);
        if rebuild || (replay.is_some() && replays >= 3) {
            let why = match root_flags {
                f if f & KERNEL_DROPPED != 0 => "kernel dropped events",
                0 => "events dropped again while replaying",
                _ => "history unavailable",
            };
            relist_changed(shared, why, root_flags);
            (gap, replays) = (None, 0);
        } else if let Some(from) = replay {
            log(format!("FSEvents dropped events before we read them: replaying from {from}"));
            shared.watch(from);
            (gap, replays) = (None, replays + 1);
        } else if had_events && gap.is_none() && pace.held.is_empty() && !shared.replaying.load(Ordering::Relaxed) {
            shared.live.write().unwrap().as_mut().unwrap().synced_at = crate::query::now_secs();
        }
        if let Some(l) = shared.live.read().unwrap().as_ref() {
            l.names_cache.trim_if_idle(Duration::from_secs(60));
        }
        if now_ns().saturating_sub(LAST_SEARCH.load(Ordering::Relaxed)) > 60_000_000_000 && crate::alloc::kept() {
            pool().spawn_broadcast(|_| crate::alloc::drain());
        }
        if !shared.owner() && (owner_wrote || last_follow.elapsed() > FOLLOW_EVERY) {
            last_follow = Instant::now();
            if !try_upgrade(shared) {
                shared.follow();
            }
        }
        if !shared.owner() {
            continue;
        }
        let (pending, stale) = {
            let g = shared.live.read().unwrap();
            let live = g.as_ref().unwrap();
            (live.over.len() + live.dead_count, live.event_id != live.base.event_id)
        };
        let asked = shared.save_requested.swap(false, Ordering::Relaxed);
        if asked || pending > COMPACT_PENDING || (stale && last_save.elapsed() > COMPACT_EVERY) {
            compact(shared);
            last_save = Instant::now();
        }
    }
}

#[cfg(target_os = "macos")]
pub fn default_dir(home: &str) -> PathBuf {
    Path::new(home).join("Library/Application Support/FSearch")
}

#[cfg(target_os = "linux")]
pub fn default_dir(home: &str) -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| Path::new(home).join(".local/share")).join("fsearch")
}
