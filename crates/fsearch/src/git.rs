use crate::frecency::decay;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

pub const MODIFIED: u8 = 1;

pub const STAGED: u8 = 2;
pub const ADDED: u8 = 4;
pub const RENAMED: u8 = 8;
pub const DELETED: u8 = 16;
pub const UNTRACKED: u8 = 32;
pub const IGNORED: u8 = 64;
pub const CONFLICT: u8 = 128;

pub fn word(s: u8) -> Option<&'static str> {
    Some(match s {
        0 => return None,
        s if s & CONFLICT != 0 => "conflict",
        s if s & UNTRACKED != 0 => "untracked",
        s if s & IGNORED != 0 => "ignored",
        s if s & MODIFIED != 0 => "modified",
        s if s & ADDED != 0 => "added",
        s if s & RENAMED != 0 => "renamed",
        s if s & DELETED != 0 => "deleted",
        _ => "staged",
    })
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sel {
    Modified,
    Untracked,
    Staged,
    Clean,
    Ignored,
    Tracked,
    Conflict,
}

impl Sel {
    pub fn parse(v: &str) -> Option<Sel> {
        let all = [
            ("modified", Sel::Modified),
            ("untracked", Sel::Untracked),
            ("staged", Sel::Staged),
            ("clean", Sel::Clean),
            ("ignored", Sel::Ignored),
            ("tracked", Sel::Tracked),
            ("conflict", Sel::Conflict),
        ];
        all.into_iter().find(|(n, _)| !v.is_empty() && n.starts_with(v)).map(|(_, s)| s)
    }

    pub fn test(self, s: Option<u8>) -> bool {
        let Some(s) = s else { return false };
        match self {
            Sel::Modified => s & !IGNORED != 0,
            Sel::Untracked => s & UNTRACKED != 0,
            Sel::Staged => s & STAGED != 0,
            Sel::Clean => s == 0,
            Sel::Ignored => s & IGNORED != 0,
            Sel::Tracked => s & (UNTRACKED | IGNORED) == 0,
            Sel::Conflict => s & CONFLICT != 0,
        }
    }

    pub fn listed(self) -> bool {
        !matches!(self, Sel::Clean | Sel::Tracked)
    }
}

#[derive(Default, Clone)]
pub struct Snap {
    pub files: HashMap<Box<[u8]>, u8>,
    pub dirs: HashMap<Box<[u8]>, u8>,

    pub history: HashMap<Box<[u8]>, f32>,
    pub at: u32,

    head: Vec<u8>,

    untrusted: bool,
}

impl Snap {
    pub fn status(&self, rel: &[u8]) -> u8 {
        if rel == b".git" || rel.starts_with(b".git/") {
            return IGNORED;
        }
        if let Some(&s) = self.files.get(rel) {
            return s;
        }
        let mut p = rel;
        loop {
            if let Some(&s) = self.dirs.get(p) {
                return s;
            }
            match p.iter().rposition(|&b| b == b'/') {
                Some(i) => p = &p[..i],
                None => return 0,
            }
        }
    }
}

pub struct Repo {
    pub root: Vec<u8>,
    snap: RwLock<Option<Arc<Snap>>>,
    loaded: (Mutex<bool>, Condvar),

    pending: Mutex<(HashMap<Vec<u8>, bool>, bool)>,
    last_event: AtomicU64,

    used: AtomicU64,
    queued: AtomicBool,
}

impl Repo {
    pub fn snap(&self, wait: Duration) -> Option<Arc<Snap>> {
        self.used.store(ms(), Ordering::Relaxed);
        if wait > Duration::ZERO {
            let (m, cv) = &self.loaded;
            drop(cv.wait_timeout_while(m.lock().unwrap(), wait, |done| !*done).unwrap());
        }
        self.snap.read().unwrap().clone()
    }

    fn rel<'a>(&self, path: &'a [u8]) -> Option<&'a [u8]> {
        let r = path.strip_prefix(self.root.as_slice())?;
        match r {
            [] => Some(r),
            [b'/', rest @ ..] => Some(rest),
            _ => None,
        }
    }
}

pub struct Git {
    home: Vec<u8>,
    bin: Option<std::path::PathBuf>,
    repos: Mutex<Vec<Arc<Repo>>>,

    dirs: Mutex<HashMap<Vec<u8>, Option<Arc<Repo>>>>,
    work: Mutex<Sender<Arc<Repo>>>,
}

const QUIET: Duration = Duration::from_millis(100);

const IDLE_MS: u64 = 20 * 60 * 1000;

const IDLE_CHECK: Duration = Duration::from_secs(5 * 60);

const GIT_TIMEOUT: Duration = Duration::from_secs(60);

static RUNNING: AtomicI32 = AtomicI32::new(0);

pub fn kill_on_term() {
    extern "C" fn on_term(sig: i32) {
        let p = RUNNING.load(Ordering::Relaxed);
        if p > 0 {
            unsafe { libc::kill(-p, libc::SIGKILL) };
        }
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }
    for sig in [libc::SIGTERM, libc::SIGINT] {
        unsafe { libc::signal(sig, on_term as extern "C" fn(i32) as libc::sighandler_t) };
    }
}

const MAX_PATHSPECS: usize = 2000;

fn ms() -> u64 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

impl Git {
    pub fn new(home: &str) -> Arc<Git> {
        let (tx, rx) = std::sync::mpsc::channel();
        let g =
            Arc::new(Git { home: home.as_bytes().to_vec(), bin: git_bin(), repos: Mutex::default(), dirs: Mutex::default(), work: Mutex::new(tx) });
        let w = g.clone();
        std::thread::Builder::new().name("fsearch-git".into()).spawn(move || w.worker(rx)).expect("spawn");
        g
    }

    pub fn repo(&self, path: &[u8]) -> Option<Arc<Repo>> {
        self.bin.as_ref()?;
        let dir = &path[..path.iter().rposition(|&b| b == b'/').unwrap_or(0)];
        let mut memo = self.dirs.lock().unwrap();
        let mut seen = Vec::new();
        let mut d = dir;
        let found = loop {
            if let Some(r) = memo.get(d) {
                break r.clone();
            }
            seen.push(d.to_vec());
            if d.len() > 1 && d != self.home.as_slice() && std::fs::symlink_metadata(OsStr::from_bytes(&[d, b"/.git"].concat())).is_ok() {
                break Some(self.add(d));
            }
            match d.iter().rposition(|&b| b == b'/') {
                Some(i) if d.len() > 1 => d = &d[..i.max(1)],
                _ => break None,
            }
        };
        if memo.len() > 100_000 {
            memo.clear();
        }
        for s in seen {
            memo.insert(s, found.clone());
        }
        found
    }

    pub fn load(&self, path: &[u8]) -> Option<Arc<Repo>> {
        let r = self.repo(path)?;
        r.used.store(ms(), Ordering::Relaxed);
        if r.snap.read().unwrap().is_none() {
            self.kick(&r);
        }
        Some(r)
    }

    fn add(&self, root: &[u8]) -> Arc<Repo> {
        let mut repos = self.repos.lock().unwrap();
        if let Some(r) = repos.iter().find(|r| r.root == root) {
            return r.clone();
        }
        let r = Arc::new(Repo {
            root: root.to_vec(),
            snap: RwLock::new(None),
            loaded: (Mutex::new(false), Condvar::new()),
            pending: Mutex::default(),
            last_event: AtomicU64::new(0),
            used: AtomicU64::new(ms()),
            queued: AtomicBool::new(false),
        });
        repos.push(r.clone());
        r
    }

    pub fn status(&self, path: &[u8]) -> Option<u8> {
        let r = self.repo(path)?;
        let s = r.snap(Duration::ZERO)?;
        Some(s.status(r.rel(path)?))
    }

    pub fn history(&self, path: &[u8], now: u32) -> f32 {
        let Some(r) = self.repo(path) else { return 0.0 };
        let Some(s) = r.snap(Duration::ZERO) else { return 0.0 };
        r.rel(path).and_then(|rel| s.history.get(rel)).map_or(0.0, |&h| h * decay(now.saturating_sub(s.at)))
    }

    pub fn history_under(&self, dir: &[u8], now: u32) -> Vec<(Vec<u8>, f32)> {
        let Some(r) = self.repo(&[dir, b"/."].concat()) else { return Vec::new() };
        let Some(s) = r.snap(Duration::ZERO) else { return Vec::new() };
        let d = decay(now.saturating_sub(s.at));
        let paths = s.history.iter().map(|(rel, &h)| ([&r.root[..], b"/", rel].concat(), h * d));
        paths.filter(|(p, _)| p.starts_with(dir) && p.get(dir.len()) == Some(&b'/')).collect()
    }

    pub fn known_under(&self, dir: &[u8]) -> Vec<Arc<Repo>> {
        self.repos.lock().unwrap().iter().filter(|r| r.root.starts_with(dir) && r.root.get(dir.len()).is_none_or(|&b| b == b'/')).cloned().collect()
    }

    pub fn changed(&self, dirs: &[(&[u8], bool)]) {
        {
            let mut memo = self.dirs.lock().unwrap();
            let stale = dirs.iter().any(|(d, _)| {
                memo.get(*d).is_some_and(|r| {
                    let root = r.as_ref().is_some_and(|r| r.root == *d);
                    root != std::fs::symlink_metadata(OsStr::from_bytes(&[d, &b"/.git"[..]].concat())).is_ok()
                })
            });
            if stale {
                memo.clear();
            }
        }
        let repos = self.repos.lock().unwrap().clone();
        let mut hit: Vec<&Arc<Repo>> = Vec::new();
        for &(d, below) in dirs {
            let above = |r: &&Arc<Repo>| below && r.root.starts_with(d) && (d.ends_with(b"/") || r.root.get(d.len()) == Some(&b'/'));
            let mut todo: Vec<(&Arc<Repo>, &[u8])> = repos.iter().filter(above).map(|r| (r, &b""[..])).collect();

            todo.extend(repos.iter().filter_map(|r| Some((r, r.rel(d)?))).max_by_key(|(r, _)| r.root.len()));
            for (r, rel) in todo {
                let mut p = r.pending.lock().unwrap();
                if rel == b".git" || rel.starts_with(b".git/refs") || rel == b".git/info" {
                    p.1 = true;
                } else if !rel.starts_with(b".git/") {
                    *p.0.entry(rel.to_vec()).or_insert(false) |= below;
                } else {
                    continue;
                }
                if !hit.iter().any(|h| Arc::ptr_eq(h, r)) {
                    hit.push(r);
                }
            }
        }
        for r in hit {
            r.last_event.store(ms(), Ordering::Relaxed);
            self.kick(r);
        }
    }

    fn kick(&self, r: &Arc<Repo>) {
        if !r.queued.swap(true, Ordering::Relaxed) {
            let _ = self.work.lock().unwrap().send(r.clone());
        }
    }

    fn worker(&self, rx: Receiver<Arc<Repo>>) {
        let mut queue: Vec<Arc<Repo>> = Vec::new();
        loop {
            let wait = match queue.is_empty() {
                true if self.repos.lock().unwrap().is_empty() => None,
                true => Some(IDLE_CHECK),
                false => Some(QUIET),
            };
            let got = match wait {
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some(t) => rx.recv_timeout(t),
            };
            match got {
                Ok(r) => queue.push(r),
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
            queue.extend(rx.try_iter());
            queue.dedup_by(|a, b| Arc::ptr_eq(a, b));
            let now = ms();
            let ripe = |r: &Arc<Repo>| {
                r.snap.read().unwrap().is_none() || now.saturating_sub(r.last_event.load(Ordering::Relaxed)) >= QUIET.as_millis() as u64
            };
            while let Some(i) = queue.iter().position(ripe) {
                let r = queue.swap_remove(i);
                r.queued.store(false, Ordering::Relaxed);
                self.refresh(&r);
            }
            self.drop_idle();
        }
    }

    fn drop_idle(&self) {
        let now = ms();
        let mut repos = self.repos.lock().unwrap();
        let idle: Vec<Arc<Repo>> = repos.iter().filter(|r| now.saturating_sub(r.used.load(Ordering::Relaxed)) > IDLE_MS).cloned().collect();
        if idle.is_empty() {
            return;
        }
        repos.retain(|r| !idle.iter().any(|i| Arc::ptr_eq(i, r)));
        drop(repos);
        self.dirs.lock().unwrap().retain(|_, r| r.as_ref().is_none_or(|r| !idle.iter().any(|i| Arc::ptr_eq(i, r))));
    }

    fn refresh(&self, r: &Repo) {
        let (dirs, head) = std::mem::take(&mut *r.pending.lock().unwrap());
        let old = r.snap.read().unwrap().clone();
        let t = Instant::now();
        let new = match old {
            None => self.read_all(r),
            Some(old) => self.update(r, &old, dirs, head),
        };
        if let Some(s) = new {
            *r.snap.write().unwrap() = Some(Arc::new(s));
        }
        let first = !std::mem::replace(&mut *r.loaded.0.lock().unwrap(), true);
        r.loaded.1.notify_all();
        if first || t.elapsed() > Duration::from_millis(500) {
            fsearch::diag_line(format!("git: read {} in {:.2?}", String::from_utf8_lossy(&r.root), t.elapsed()));
        }
    }

    fn read_all(&self, r: &Repo) -> Option<Snap> {
        let mut s = Snap::default();
        if !self.trusted(&r.root) {
            s.untrusted = true;
            return Some(s);
        }
        for (path, bits) in parse(&self.run(&r.root, &status_args(), &[])?) {
            insert(&mut s, path, bits);
        }
        s.head = self.head(&r.root);
        (s.history, s.at) = self.history_of(&r.root);
        Some(s)
    }

    fn trusted(&self, root: &[u8]) -> bool {
        let filters = self.run(root, &["config", "--local", "--includes", "--get-regexp", r"^filter\..*\.(clean|smudge|process)$"], &[]);
        if filters.as_ref().is_some_and(|f| !f.is_empty()) {
            fsearch::diag_line(format!("git: {} configures its own filters; not reading its status", String::from_utf8_lossy(root)));
            return false;
        }
        true
    }

    fn head(&self, root: &[u8]) -> Vec<u8> {
        self.run(root, &["rev-parse", "-q", "--verify", "HEAD"], &[]).unwrap_or_default()
    }

    fn update(&self, r: &Repo, old: &Snap, dirs: HashMap<Vec<u8>, bool>, head: bool) -> Option<Snap> {
        if old.untrusted {
            return if head { self.read_all(r) } else { None };
        }
        let mut s = old.clone();

        let mut dirs = dirs;
        for (d, rec) in dirs.iter_mut() {
            let gi = [&r.root[..], b"/", d, if d.is_empty() { b"" } else { b"/" }, b".gitignore"].concat();
            let fresh = std::fs::metadata(OsStr::from_bytes(&gi))
                .and_then(|m| m.modified())
                .is_ok_and(|m| m.elapsed().is_ok_and(|e| e < Duration::from_secs(5)));
            *rec |= fresh;
        }
        if dirs.iter().any(|(d, rec)| *rec && d.is_empty()) || dirs.len() > MAX_PATHSPECS {
            return self.read_all(r);
        }
        let mut specs: Vec<Vec<u8>> = Vec::new();
        for (d, rec) in &dirs {
            if *rec {
                specs.push([b":(literal)", &d[..]].concat());
            } else if d.is_empty() {
                for e in std::fs::read_dir(OsStr::from_bytes(&r.root)).into_iter().flatten().flatten() {
                    let name = e.file_name();
                    if name.as_bytes() == b".git" {
                        continue;
                    }
                    specs.push(match e.file_type().is_ok_and(|t| t.is_dir()) {
                        true => [b":(glob)", &glob_escape(name.as_bytes())[..]].concat(),
                        false => [b":(literal)", name.as_bytes()].concat(),
                    });
                }
            } else {
                specs.push([b":(glob)", &glob_escape(d)[..], b"/*"].concat());
            }
        }

        if head {
            if !self.trusted(&r.root) {
                return Some(Snap { untrusted: true, ..Snap::default() });
            }
            let cached = self.run(&r.root, &["diff", "--cached", "--name-only", "-z", "--no-ext-diff", "--no-textconv"], &[])?;
            let now: HashSet<&[u8]> = cached.split(|&b| b == 0).filter(|p| !p.is_empty()).collect();
            let was: HashSet<&[u8]> = s.files.iter().filter(|(_, b)| **b & STAGED != 0).map(|(p, _)| &p[..]).collect();
            specs.extend(now.union(&was).map(|p| [b":(literal)", *p].concat()));
            let h = self.head(&r.root);
            if h != s.head {
                (s.history, s.at) = self.history_of(&r.root);
                s.head = h;
            }
        }
        if specs.is_empty() {
            return Some(s);
        }
        let found = parse(&self.run(&r.root, &status_args(), &specs)?);

        let covered = |p: &[u8]| {
            let parent = &p[..p.iter().rposition(|&b| b == b'/').unwrap_or(0)];
            dirs.iter().any(|(d, rec)| if *rec { d.is_empty() || p == &d[..] || p.starts_with(&[&d[..], b"/"].concat()) } else { parent == &d[..] })
        };
        let restaged: HashSet<Vec<u8>> = specs.iter().filter_map(|s| s.strip_prefix(b":(literal)")).map(<[u8]>::to_vec).collect();
        s.files.retain(|p, _| !covered(p) && !restaged.contains(&p[..]));
        s.dirs.retain(|p, _| !covered(p));
        for (path, bits) in found {
            insert(&mut s, path, bits);
        }
        Some(s)
    }

    fn history_of(&self, root: &[u8]) -> (HashMap<Box<[u8]>, f32>, u32) {
        let (mut out, now) = (HashMap::new(), fsearch::query::now_secs());
        let me = self.run(root, &["config", "user.email"], &[]).map(|e| String::from_utf8_lossy(&e).trim().to_string());
        let Some(me) = me.filter(|e| !e.is_empty()) else { return (out, now) };
        let author = format!("--author=<{me}>");
        let args = ["log", "-F", &author, "--since=60.days", "-n", "1000", "--no-merges", "--name-only", "--format=%x00%at"];
        for commit in self.run(root, &args, &[]).unwrap_or_default().split(|&b| b == 0).filter(|c| !c.is_empty()) {
            let mut lines = commit.split(|&b| b == b'\n');
            let t: u32 = lines.next().and_then(|l| std::str::from_utf8(l).ok()?.trim().parse().ok()).unwrap_or(0);
            let files: Vec<&[u8]> = lines.filter(|l| !l.is_empty()).collect();
            if files.len() > 50 {
                continue;
            }
            let w = decay(now.saturating_sub(t));
            for f in files {
                *out.entry(f.into()).or_insert(0.0) += w;
            }
        }
        (out, now)
    }

    fn run(&self, root: &[u8], args: &[&str], specs: &[Vec<u8>]) -> Option<Vec<u8>> {
        let mut c = std::process::Command::new(self.bin.as_ref()?);
        c.arg("-C").arg(OsStr::from_bytes(root));
        let safe = ["-c", "core.fsmonitor=false", "-c", "log.showSignature=false"];
        c.args(["--no-optional-locks", "-c", "core.quotePath=false", "-c", "core.preloadIndex=false"]).args(safe).args(args);
        if !specs.is_empty() {
            c.arg("--").args(specs.iter().map(|s| OsStr::from_bytes(s)));
        }
        c.env("GIT_OPTIONAL_LOCKS", "0").env("GIT_TERMINAL_PROMPT", "0");
        c.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).stdout(std::process::Stdio::piped());
        unsafe {
            c.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                Ok(())
            });
        }
        c.process_group(0);
        let mut child = c.spawn().ok()?;
        RUNNING.store(child.id() as i32, Ordering::Relaxed);
        let mut stdout = child.stdout.take()?;

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = std::io::Read::read_to_end(&mut stdout, &mut out);
            let _ = tx.send(out);
        });
        let out = rx.recv_timeout(GIT_TIMEOUT).ok();

        RUNNING.store(0, Ordering::Relaxed);
        if out.is_none() {
            let _ = child.kill();
        }
        let ok = child.wait().is_ok_and(|s| s.success());
        out.filter(|_| ok)
    }
}

fn status_args() -> [&'static str; 6] {
    ["status", "--porcelain=v2", "-z", "--ignored=traditional", "--untracked-files=normal", "--ignore-submodules=all"]
}

fn insert(s: &mut Snap, path: Vec<u8>, bits: u8) {
    match path.strip_suffix(b"/") {
        Some(d) => s.dirs.insert(d.into(), bits),
        None => s.files.insert(path.into(), bits),
    };
}

fn parse(out: &[u8]) -> Vec<(Vec<u8>, u8)> {
    let mut v = Vec::new();
    let mut recs = out.split(|&b| b == 0);
    while let Some(rec) = recs.next() {
        let field = |n: usize| rec.splitn(n + 1, |&b| b == b' ').nth(n);
        let (path, bits) = match rec.first() {
            Some(b'1') => (field(8), xy(rec)),
            Some(b'2') => {
                recs.next();
                (field(9), xy(rec))
            }
            Some(b'u') => (field(10), CONFLICT),
            Some(b'?') => (rec.get(2..), UNTRACKED),
            Some(b'!') => (rec.get(2..), IGNORED),
            _ => (None, 0),
        };
        if let Some(p) = path.filter(|p| !p.is_empty()) {
            v.push((p.to_vec(), bits));
        }
    }
    v
}

fn xy(rec: &[u8]) -> u8 {
    let (x, y) = (rec.get(2).copied().unwrap_or(b'.'), rec.get(3).copied().unwrap_or(b'.'));
    let staged = match x {
        b'M' | b'T' => STAGED,
        b'A' => STAGED | ADDED,
        b'D' => STAGED | DELETED,
        b'R' | b'C' => STAGED | RENAMED,
        _ => 0,
    };
    let work = match y {
        b'M' | b'T' => MODIFIED,
        b'D' => DELETED,
        _ => 0,
    };
    staged | work
}

fn glob_escape(p: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.len());
    for &b in p {
        if matches!(b, b'*' | b'?' | b'[' | b']' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out
}

fn git_bin() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::env::split_paths(&path).filter(|d| d != std::path::Path::new("/usr/bin"));
    dirs.chain(["/Library/Developer/CommandLineTools/usr/bin".into(), "/Applications/Xcode.app/Contents/Developer/usr/bin".into()])
        .map(|d| d.join("git"))
        .find(|g| g.is_file())
}
