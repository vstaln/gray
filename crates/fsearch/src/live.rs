use crate::index::{Index, enc_size};
use crate::walk::{self, FLAG_MOUNT, KIND_DIR, Listing, NONE, RawEnt};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy)]
pub struct OEnt {
    pub kind: u8,
    pub size: u64,
    pub mtime: u32,

    pub mask: u64,

    pub prior: i8,

    slot: u32,
}

impl OEnt {
    pub fn new(path: &[u8], kind: u8, size: u64, mtime: u32) -> OEnt {
        let name = &path[path.iter().rposition(|&b| b == b'/').map_or(0, |p| p + 1)..];
        OEnt { kind, size, mtime, mask: crate::index::name_mask(name), prior: 0, slot: 0 }
    }
}

#[derive(Default)]
pub struct Flat {
    pub masks: Vec<u64>,
    pub items: Vec<(Vec<u8>, OEnt)>,
    free: Vec<u32>,
}

impl Flat {
    fn set(&mut self, path: &[u8], e: OEnt) {
        let s = e.slot as usize;
        if s == self.masks.len() {
            self.masks.push(0);
            self.items.push((Vec::new(), e));
        }
        self.masks[s] = e.mask;
        if self.items[s].0 != path {
            self.items[s].0 = path.to_vec();
        }
        self.items[s].1 = e;
    }

    fn clear(&mut self, e: OEnt) {
        self.masks[e.slot as usize] = 0;
        self.items[e.slot as usize].0 = Vec::new();
        self.free.push(e.slot);
    }
}

pub struct Live {
    pub base: Index,

    pub root: Vec<u8>,
    dead: Vec<u64>,
    pub dead_count: usize,
    pub over: BTreeMap<Vec<u8>, OEnt>,

    pub flat: Flat,

    pub event_id: u64,

    pub trees: Vec<Vec<u8>>,

    pub names_cache: crate::query::NameCache,

    pub synced_at: u32,

    pub root_missing: AtomicBool,

    priors: HashMap<Vec<u8>, i8>,
}

pub struct Fetched {
    path: Vec<u8>,
    recursive: bool,
    blocked: bool,
    attrs: Option<OEnt>,
    listing: Option<Listing>,
    scans: HashMap<Vec<u8>, Vec<Listing>>,
}

pub enum Applied {
    Done,

    Rebuild,
}

impl Live {
    pub fn new(base: Index) -> Live {
        base.prefault();
        Live::lazy(base)
    }

    pub fn lazy(base: Index) -> Live {
        let words = base.n.div_ceil(64);
        let event_id = base.event_id;
        let synced_at = base.synced_at;
        Live {
            base,
            root: b"/".to_vec(),
            dead: vec![0; words],
            dead_count: 0,
            over: BTreeMap::new(),
            flat: Flat::default(),
            event_id,
            trees: Vec::new(),
            names_cache: Default::default(),
            synced_at,
            root_missing: AtomicBool::new(false),
            priors: HashMap::new(),
        }
    }

    pub fn rooted(mut self, root: &[u8]) -> Live {
        self.root = root.to_vec();
        self
    }

    fn under_root(&self, p: &[u8]) -> bool {
        in_tree(p, &self.root)
    }

    fn put(&mut self, path: Vec<u8>, mut e: OEnt) {
        let cut = path.iter().rposition(|&b| b == b'/').unwrap_or(0);
        e.prior = match self.priors.get(&path[..cut]) {
            Some(&p) => p,
            None => {
                let mut up = &path[..cut];
                let mut prior = 0;
                while !up.is_empty() {
                    if let Some(d) = self.base.lookup(up).and_then(|e| self.base.dir_of(e)) {
                        prior = self.base.dir_prior()[d as usize];
                        break;
                    }
                    up = &up[..up.iter().rposition(|&b| b == b'/').unwrap_or(0)];
                }
                self.priors.insert(path[..cut].to_vec(), prior);
                prior
            }
        };
        e.slot = match self.over.get(&path) {
            Some(old) => old.slot,
            None => self.flat.free.pop().unwrap_or(self.flat.masks.len() as u32),
        };
        self.flat.set(&path, e);
        self.over.insert(path, e);
    }

    #[inline]
    pub fn is_dead(&self, i: u32) -> bool {
        self.dead[i as usize >> 6] & (1 << (i & 63)) != 0
    }

    fn kill(&mut self, i: u32) {
        let w = &mut self.dead[i as usize >> 6];
        if *w & (1 << (i & 63)) == 0 {
            *w |= 1 << (i & 63);
            self.dead_count += 1;
        }
    }

    fn kill_subtree(&mut self, e: u32) {
        self.kill(e);
        if let Some(d) = self.base.dir_of(e) {
            let (a, b) = (self.base.dir_start(d as usize), self.base.dir_end(d as usize));
            for i in a..b {
                self.kill(i);
            }
        }
    }

    fn drop_over_subtree(&mut self, path: &[u8]) {
        self.priors.remove(path);
        if let Some(o) = self.over.remove(path) {
            self.flat.clear(o);
        }
        let (lo, hi) = subtree_bounds(path);
        let keys: Vec<Vec<u8>> = self.over.range(lo..hi).map(|(k, _)| k.clone()).collect();
        for k in keys {
            self.priors.remove(&k);
            if let Some(o) = self.over.remove(&k) {
                self.flat.clear(o);
            }
        }

        self.priors.retain(|k, _| !in_tree(k, path));
    }

    fn base_alive(&self, path: &[u8]) -> Option<u32> {
        self.base.lookup(path).filter(|&e| !self.is_dead(e))
    }

    fn remove_path(&mut self, path: &[u8]) {
        self.trees.push(path.to_vec());
        if let Some(e) = self.base_alive(path) {
            self.kill_subtree(e);
        }
        self.drop_over_subtree(path);
    }

    pub fn apply_dir(&mut self, path: &[u8], recursive: bool) -> Applied {
        let f = self.fetch(path, recursive);
        self.apply(f)
    }

    pub fn fetch(&self, path: &[u8], recursive: bool) -> Fetched {
        let p = normalize(path);
        let mut f = Fetched { path: p, recursive, blocked: false, attrs: None, listing: None, scans: HashMap::new() };

        if walk::blocked(&f.path) {
            f.blocked = true;
            return f;
        }

        if !self.under_root(&f.path) {
            f.blocked = true;
            return f;
        }

        if self.root_missing.load(Ordering::Relaxed) && f.path != self.root {
            if lstat(&self.root).is_none() {
                f.blocked = true;
                return f;
            }
            f.path = self.root.clone();
        }
        if recursive {
            if f.path != b"/" {
                f.attrs = lstat(&f.path);
                if f.attrs.is_some_and(|a| a.kind & 3 == KIND_DIR && a.kind & FLAG_MOUNT == 0) {
                    f.scans.insert(f.path.clone(), walk::scan(&f.path, 4));
                }
            }
            return f;
        }
        let Some(listing) = walk::list_one(&f.path) else {
            f.attrs = lstat(&f.path);
            return f;
        };

        let cur = self.current_children(&f.path);
        for r in &listing.ents {
            if r.kind & 3 != KIND_DIR || r.kind & FLAG_MOUNT != 0 {
                continue;
            }
            let name = &listing.names[r.name_off as usize..][..r.name_len as usize];
            let was_dir = match cur.get(name) {
                Some(Some(c)) => self.base.kind(*c as usize) & 3 == KIND_DIR,
                Some(None) => self.over.get(&join(&f.path, name)).is_some_and(|o| o.kind & 3 == KIND_DIR),
                None => false,
            };
            if !was_dir {
                let child = join(&f.path, name);
                let ls = walk::scan(&child, 4);
                f.scans.insert(child, ls);
            }
        }
        f.listing = Some(listing);
        f
    }

    fn current_children(&self, p: &[u8]) -> HashMap<Vec<u8>, Option<u32>> {
        let mut cur: HashMap<Vec<u8>, Option<u32>> = HashMap::new();
        if let Some(d) = self.base_alive(p).and_then(|e| self.base.dir_of(e)) {
            for c in self.base.children(d) {
                if !self.is_dead(c as u32) {
                    cur.insert(self.base.name(c).to_vec(), Some(c as u32));
                }
            }
        }
        let (lo, hi) = subtree_bounds(p);
        let plen = lo.len();
        for k in self.over.range(lo..hi).map(|(k, _)| k) {
            if !k[plen..].contains(&b'/') {
                cur.insert(k[plen..].to_vec(), None);
            }
        }
        cur
    }

    pub fn apply(&mut self, mut f: Fetched) -> Applied {
        if f.blocked {
            return Applied::Done;
        }
        let p = std::mem::take(&mut f.path);
        if f.recursive && p == b"/" {
            self.root_missing.store(false, Ordering::Relaxed);
            return Applied::Rebuild;
        }
        if p == self.root {
            if f.attrs.is_none() && f.listing.is_none() {
                self.root_missing.store(true, Ordering::Relaxed);
                return Applied::Done;
            }
            if self.root_missing.swap(false, Ordering::Relaxed) {
                let rec = f.recursive;
                self.synced_at = if rec { crate::query::now_secs() } else { 0 };
                let r = self.apply_diff(p, f);
                return if rec { r } else { Applied::Rebuild };
            }
        } else if self.root_missing.load(Ordering::Relaxed) {
            return Applied::Done;
        }
        self.apply_diff(p, f)
    }

    fn apply_diff(&mut self, p: Vec<u8>, mut f: Fetched) -> Applied {
        if f.recursive {
            self.remove_path(&p);
            if let Some(attrs) = f.attrs {
                let scan = f.scans.remove(&p);
                self.add_new(p, attrs, scan);
            }
            return Applied::Done;
        }
        let Some(listing) = f.listing else {
            if f.attrs.is_none() {
                self.remove_path(&p);
            }
            return Applied::Done;
        };
        let mut cur = self.current_children(&p);
        for r in &listing.ents {
            let name = &listing.names[r.name_off as usize..][..r.name_len as usize];
            let child = join(&p, name);
            let now = OEnt::new(name, r.kind, r.size, r.mtime);
            match cur.remove(name) {
                None => {
                    let scan = f.scans.remove(&child);
                    self.add_new(child, now, scan)
                }
                Some(Some(c)) => {
                    let c = c as usize;
                    let was_kind = self.base.kind(c);
                    if was_kind & 3 != now.kind & 3 {
                        self.kill_subtree(c as u32);

                        if was_kind & 3 == KIND_DIR {
                            self.drop_over_subtree(&child);
                        }
                        let scan = f.scans.remove(&child);
                        self.add_new(child, now, scan);
                    } else if now.kind & 3 != KIND_DIR && (self.base.size_raw(c) != enc_size(now.size) || self.base.mtime(c) != now.mtime) {
                        self.kill(c as u32);
                        self.put(child, now);
                    }
                }
                Some(None) => {
                    let old = self.over[&child];
                    if old.kind & 3 != now.kind & 3 {
                        self.drop_over_subtree(&child);
                        let scan = f.scans.remove(&child);
                        self.add_new(child, now, scan);
                    } else if (old.kind, old.size, old.mtime) != (now.kind, now.size, now.mtime) {
                        self.put(child, now);
                    }
                }
            }
        }
        for (name, c) in cur {
            let child = join(&p, &name);
            match c {
                Some(c) => {
                    if self.base.kind(c as usize) & 3 == KIND_DIR {
                        self.drop_over_subtree(&child);
                        self.trees.push(child);
                    }
                    self.kill_subtree(c)
                }
                None => {
                    if self.over.get(&child).is_some_and(|o| o.kind & 3 == KIND_DIR) {
                        self.trees.push(child.clone());
                    }
                    self.drop_over_subtree(&child)
                }
            }
        }
        Applied::Done
    }

    fn add_new(&mut self, path: Vec<u8>, e: OEnt, scan: Option<Vec<Listing>>) {
        let is_dir = e.kind & 3 == KIND_DIR && e.kind & FLAG_MOUNT == 0;
        self.put(path.clone(), e);
        if !is_dir {
            return;
        }
        self.trees.push(path.clone());
        let ls = scan.unwrap_or_else(|| walk::scan(&path, 4));
        for_each_path(&ls, &path, |p, r| {
            let e = OEnt::new(&p, r.kind, r.size, r.mtime);
            self.put(p, e);
        });
    }

    pub fn changed_dirs(&self, since: u32) -> Vec<Vec<u8>> {
        self.changed_dirs_in(since, &[])
    }

    pub fn changed_dirs_in(&self, since: u32, scopes: &[Vec<u8>]) -> Vec<Vec<u8>> {
        use rayon::prelude::*;
        let idx = &self.base;
        let de = idx.dir_entries();
        let in_scope = |p: &[u8]| scopes.is_empty() || scopes.iter().any(|s| crate::engine::under(p, s));
        let check = |p: &[u8]| self.under_root(p) && !walk::blocked(p) && in_scope(p) && lstat(p).is_some_and(|o| o.mtime >= since);
        let pool = stat_pool();
        let mut out: Vec<Vec<u8>> = pool.install(|| {
            (1..idx.d)
                .into_par_iter()
                .with_min_len(1024)
                .filter(|&k| !self.is_dead(de.get(k)))
                .map_init(Vec::new, |buf, k| {
                    idx.path(de.get(k) as usize, buf);
                    check(buf).then(|| buf.clone())
                })
                .flatten()
                .collect()
        });
        out.extend(self.over.iter().filter(|(p, o)| o.kind & 3 == KIND_DIR && o.kind & FLAG_MOUNT == 0 && check(p)).map(|(p, _)| p.clone()));
        if in_scope(&self.root) && lstat(&self.root).is_some_and(|o| o.mtime >= since) {
            out.push(self.root.clone());
        }
        out.sort();
        out
    }

    pub fn to_listings(&self) -> Vec<Listing> {
        let mut by_parent: HashMap<&[u8], Vec<(&[u8], OEnt)>> = HashMap::new();
        for (k, v) in &self.over {
            let cut = k.iter().rposition(|&b| b == b'/').unwrap_or(0);
            let parent: &[u8] = if cut == 0 { b"/" } else { &k[..cut] };
            by_parent.entry(parent).or_default().push((&k[cut + 1..], *v));
        }
        let mut out = Vec::new();

        let mut stack: Vec<(Option<u32>, Vec<u8>, u32)> = vec![(Some(0), b"/".to_vec(), 0)];
        let mut next = 1u32;
        while let Some((bd, path, id)) = stack.pop() {
            let mut l = Listing { id, names: Vec::new(), ents: Vec::new() };
            let mut push = |l: &mut Listing, name: &[u8], kind: u8, size: u64, mtime: u32, child_path: Option<Vec<u8>>, bdir: Option<u32>| {
                let child = match child_path {
                    Some(cp) => {
                        let c = next;
                        next += 1;
                        stack.push((bdir, cp, c));
                        c
                    }
                    None => NONE,
                };
                l.ents.push(RawEnt { name_off: l.names.len() as u32, name_len: name.len() as u16, kind, size, mtime, child });
                l.names.extend_from_slice(name);
            };
            if let Some(d) = bd {
                for c in self.base.children(d) {
                    if self.is_dead(c as u32) {
                        continue;
                    }
                    let name = self.base.name(c);
                    let k = self.base.kind(c);
                    let sub = self.base.dir_of(c as u32);
                    let cp = sub.map(|_| join(&path, &name));
                    push(&mut l, &name, k, self.base.size_of(c), self.base.mtime(c), cp, sub);
                }
            }
            if let Some(kids) = by_parent.get(path.as_slice()) {
                for &(name, o) in kids {
                    let descend = o.kind & 3 == KIND_DIR && o.kind & FLAG_MOUNT == 0;
                    push(&mut l, name, o.kind, o.size, o.mtime, descend.then(|| join(&path, name)), None);
                }
            }
            out.push(l);
        }
        out
    }
}

pub fn stat_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| rayon::ThreadPoolBuilder::new().num_threads(12).start_handler(|_| crate::no_materialize()).build().unwrap())
}

pub fn for_each_path(ls: &[Listing], root: &[u8], mut f: impl FnMut(Vec<u8>, &RawEnt)) {
    let mut by_id: HashMap<u32, &Listing> = HashMap::with_capacity(ls.len());
    for l in ls {
        by_id.insert(l.id, l);
    }
    let mut stack = vec![(0u32, root.to_vec())];
    while let Some((id, path)) = stack.pop() {
        let Some(l) = by_id.get(&id) else { continue };
        for r in &l.ents {
            let p = join(&path, &l.names[r.name_off as usize..][..r.name_len as usize]);
            if r.child != NONE {
                stack.push((r.child, p.clone()));
            }
            f(p, r);
        }
    }
}

pub(crate) fn normalize(path: &[u8]) -> Vec<u8> {
    let mut p = path.to_vec();
    while p.len() > 1 && p.last() == Some(&b'/') {
        p.pop();
    }
    p
}

pub fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(dir.len() + 1 + name.len());
    p.extend_from_slice(dir);
    if dir != b"/" {
        p.push(b'/');
    }
    p.extend_from_slice(name);
    p
}

fn in_tree(p: &[u8], dir: &[u8]) -> bool {
    p == dir || (p.starts_with(dir) && (dir == b"/" || p[dir.len()] == b'/'))
}

fn subtree_bounds(path: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let lo = join(path, b"");
    let mut hi = lo.clone();
    *hi.last_mut().unwrap() += 1;
    (lo, hi)
}

pub fn lstat(path: &[u8]) -> Option<OEnt> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::lstat(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let kind = match st.st_mode & libc::S_IFMT {
        libc::S_IFREG => walk::KIND_FILE,
        libc::S_IFDIR => KIND_DIR,
        libc::S_IFLNK => walk::KIND_LINK,
        _ => walk::KIND_OTHER,
    };
    Some(OEnt::new(path, kind, st.st_size as u64, st.st_mtime.clamp(0, u32::MAX as i64) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(id: u32, ents: &[(&[u8], u8, u32)]) -> Listing {
        let mut l = Listing { id, names: Vec::new(), ents: Vec::new() };
        for &(name, kind, child) in ents {
            l.ents.push(RawEnt { name_off: l.names.len() as u32, name_len: name.len() as u16, kind, size: 0, mtime: 0, child });
            l.names.extend_from_slice(name);
        }
        l
    }

    fn base() -> Index {
        Index::build(
            vec![
                listing(0, &[(b"a", KIND_DIR, 1)]),
                listing(1, &[(b"b", KIND_DIR, 2), (b"bb", KIND_DIR, 3)]),
                listing(2, &[(b"x", walk::KIND_FILE, NONE)]),
                listing(3, &[(b"y", walk::KIND_FILE, NONE)]),
            ],
            0,
            0,
            b"/",
        )
    }

    fn listing_of(path: &[u8], ents: &[(&[u8], u8, u32)]) -> Fetched {
        Fetched { path: path.to_vec(), recursive: false, blocked: false, attrs: None, listing: Some(listing(0, ents)), scans: HashMap::new() }
    }

    #[test]
    fn removes_only_the_named_subtree() {
        let mut live = Live::new(base());
        live.put(b"/a/b/n".to_vec(), OEnt::new(b"n", walk::KIND_FILE, 1, 1));
        live.put(b"/a/bb/n".to_vec(), OEnt::new(b"n", walk::KIND_FILE, 1, 1));

        live.apply(listing_of(b"/a", &[(b"bb", KIND_DIR, NONE)]));
        let gone = |p: &[u8]| live.base.lookup(p).is_none_or(|e| live.is_dead(e));
        assert!(gone(b"/a/b") && gone(b"/a/b/x"));
        assert!(!gone(b"/a/bb") && !gone(b"/a/bb/y") && !gone(b"/a"));
        assert!(!live.over.contains_key(b"/a/b".as_slice()));
        assert!(!live.over.contains_key(b"/a/b/n".as_slice()));
        assert!(live.over.contains_key(b"/a/bb/n".as_slice()));
    }

    #[test]
    fn missing_root_goes_offline_and_rescans_on_return() {
        let dir = std::env::temp_dir().join(format!("fsearch-live-{}", std::process::id()));
        let root = dir.join("root");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("f"), b"x").unwrap();
        let root_b = root.as_os_str().as_encoded_bytes().to_vec();

        let comps: Vec<&[u8]> = root_b.split(|&b| b == b'/').filter(|c| !c.is_empty()).collect();
        let mut ls: Vec<Listing> = comps.iter().enumerate().map(|(i, c)| listing(i as u32, &[(*c, KIND_DIR, i as u32 + 1)])).collect();
        ls.push(listing(comps.len() as u32, &[(b"f", walk::KIND_FILE, NONE)]));
        let mut live = Live::new(Index::build(ls, 0, 1, b"/")).rooted(&root_b);
        let fpath = join(&root_b, b"f");
        let present = |live: &Live| live.over.contains_key(fpath.as_slice()) || live.base.lookup(&fpath).is_some_and(|e| !live.is_dead(e));
        assert!(matches!(live.apply_dir(&root_b, false), Applied::Done) && present(&live));

        let moved = dir.join("moved");
        std::fs::rename(&root, &moved).unwrap();
        assert!(matches!(live.apply_dir(&root_b, false), Applied::Done));
        assert!(live.root_missing.load(Ordering::Relaxed) && present(&live));
        assert!(matches!(live.apply_dir(&fpath, false), Applied::Done) && present(&live));

        std::fs::rename(&moved, &root).unwrap();
        assert!(matches!(live.apply_dir(&fpath, false), Applied::Rebuild));
        assert!(!live.root_missing.load(Ordering::Relaxed) && live.synced_at == 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
