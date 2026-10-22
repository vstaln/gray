use crate::walk::{KIND_DIR, Listing, NONE};
use memmap2::{Mmap, MmapMut};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

const MAGIC: &[u8; 8] = b"FSIDX014";

#[derive(Clone, Copy)]
enum Sec {
    NameBits,
    NameBase,
    NameOff,
    Names,
    EntName,
    Kind,
    Parent,
    ParentRank,
    ParentMap,
    Size,
    Mtime,
    DirEntry,
    DirStart,
    DirLen,
    DirEnd,
    DirPrior,
    DirParent,
    NameEntsOff,
    NameEnts,
    NamePrior,
    NameInfo,
    NameExt,
    Exts,
    Zones,
}
const NSEC: usize = 24;
#[rustfmt::skip]
const SEC_NAMES: [&str; NSEC] = [
    "NameBits", "NameBase", "NameOff", "Names", "EntName", "Kind", "Parent", "ParentRank", "ParentMap", "Size", "Mtime", "DirEntry",
    "DirStart", "DirLen", "DirEnd", "DirPrior", "DirParent", "NameEntsOff", "NameEnts", "NamePrior", "NameInfo", "NameExt", "Exts", "Zones",
];

pub struct Index {
    map: Mmap,
    pub n: usize,
    pub d: usize,

    pub u: usize,
    names_len: usize,

    pub words: usize,
    bits_len: usize,
    zones_len: usize,
    ext_slots: usize,

    n64: usize,

    blocks: usize,

    name_shift: usize,
    groups: usize,
    u1: usize,

    run_words: [usize; 7],

    pub event_id: u64,

    pub synced_at: u32,
    off: [usize; NSEC],
    plan: std::sync::OnceLock<MemoPlan>,
    counts: std::sync::OnceLock<[[u32; CLASSES]; 2]>,
    top_prior: std::sync::OnceLock<i8>,
    by_key: std::sync::OnceLock<ByKey>,
}

const PACKED: u8 = 0xFF;

const HEX_RUN: usize = 16;

pub enum Name<'a> {
    Raw(&'a [u8], &'a [u8]),

    Unpacked(Vec<u8>, usize),
}

impl std::ops::Deref for Name<'_> {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        match self {
            Name::Raw(s, _) => s,
            Name::Unpacked(v, n) => &v[..*n],
        }
    }
}

impl Name<'_> {
    #[inline]
    pub fn wide(&self) -> Option<&[u8; 64]> {
        match self {
            Name::Raw(s, map) => (s.len() <= 64).then(|| map.get(..64)?.try_into().ok()).flatten(),
            Name::Unpacked(v, n) => (*n <= 64).then(|| v[..64].try_into().unwrap()),
        }
    }
}

const LOWER: &[u8; 16] = b"0123456789abcdef";
const UPPER: &[u8; 16] = b"0123456789ABCDEF";

fn hex_run(s: &[u8], i: usize) -> (usize, bool) {
    let (mut k, mut upper) = (0, None);
    while let Some(&c) = s.get(i + k) {
        let u = match c {
            b'0'..=b'9' => None,
            b'a'..=b'f' => Some(false),
            b'A'..=b'F' => Some(true),
            _ => break,
        };
        if u.is_some() && upper.is_some_and(|x| Some(x) != u) {
            break;
        }
        upper = upper.or(u);
        k += 1;
    }
    (k, upper == Some(true))
}

fn uuid_at(s: &[u8], i: usize) -> Option<bool> {
    let u = s.get(i..i + 36)?;
    let dash = |j: usize| matches!(j, 8 | 13 | 18 | 23);
    let (mut lower, mut upper) = (true, true);
    for (j, &c) in u.iter().enumerate() {
        if dash(j) {
            if c != b'-' {
                return None;
            }
        } else {
            lower &= c.is_ascii_digit() || (b'a'..=b'f').contains(&c);
            upper &= c.is_ascii_digit() || (b'A'..=b'F').contains(&c);
        }
    }
    if lower {
        Some(false)
    } else if upper {
        Some(true)
    } else {
        None
    }
}

fn put_nibbles(out: &mut Vec<u8>, d: &[u8]) {
    let nib = |c: u8| (c as char).to_digit(16).unwrap() as u8;
    for p in d.chunks(2) {
        out.push(nib(p[0]) << 4 | p.get(1).map_or(0, |&c| nib(c)));
    }
}

fn pack_name(s: &[u8], out: &mut Vec<u8>) -> bool {
    let start = out.len();
    out.push(PACKED);
    let (mut i, mut lit, mut any) = (0, 0, false);
    let flush = |out: &mut Vec<u8>, from: usize, to: usize| {
        for c in s[from..to].chunks(0x7F) {
            out.push(c.len() as u8);
            out.extend_from_slice(c);
        }
    };
    while i < s.len() {
        if let Some(upper) = uuid_at(s, i) {
            flush(out, lit, i);
            out.push(if upper { 0xFF } else { 0xFE });
            let digits: Vec<u8> = s[i..i + 36].iter().copied().filter(|&c| c != b'-').collect();
            put_nibbles(out, &digits);
            (i, lit, any) = (i + 36, i + 36, true);
            continue;
        }
        let (k, upper) = hex_run(s, i);
        if k >= HEX_RUN {
            flush(out, lit, i);
            for c in s[i..i + k].chunks(if upper { 62 } else { 64 }) {
                out.push(if upper { 0xC0 } else { 0x80 } + c.len() as u8 - 1);
                put_nibbles(out, c);
            }
            (i, lit, any) = (i + k, i + k, true);
            continue;
        }
        i += k.max(1);
    }
    flush(out, lit, s.len());
    if !any || out.len() - start >= s.len() {
        out.truncate(start);
        return false;
    }
    true
}

fn unpack(p: &[u8]) -> Name<'static> {
    let mut v = Vec::with_capacity(2 * p.len() + 64);
    let mut i = 0;
    while i < p.len() {
        let h = p[i] as usize;
        i += 1;
        let (k, digits, uuid) = match h {
            0..=0x7F => {
                v.extend_from_slice(&p[i..i + h]);
                i += h;
                continue;
            }
            0x80..=0xBF => (h - 0x7F, LOWER, false),
            0xC0..=0xFD => (h - 0xBF, UPPER, false),
            _ => (32, if h == 0xFE { LOWER } else { UPPER }, true),
        };
        for j in 0..k {
            if uuid && matches!(j, 8 | 12 | 16 | 20) {
                v.push(b'-');
            }
            let b = p[i + j / 2];
            v.push(digits[(if j % 2 == 0 { b >> 4 } else { b & 15 }) as usize]);
        }
        i += k.div_ceil(2);
    }
    let n = v.len();
    v.resize(n.max(64), 0);
    Name::Unpacked(v, n)
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct NameInfo {
    pub key: i8,

    pub len: u8,

    pub stem: u8,

    pub head: u8,
}

pub struct ByKey {
    pub cover: i32,
    pub list: Vec<(i8, u32)>,
    pub counts: Vec<u32>,

    pub heads: Vec<Vec<u32>>,

    pub short: Vec<Vec<u32>>,
}

pub const SHORT: usize = 3;

pub struct MemoPlan {
    pub upper: Vec<u32>,

    pub chunks: Vec<(u32, std::ops::Range<u32>)>,
}

macro_rules! sec {
    ($name:ident, $s:expr, $t:ty, $len:ident) => {
        pub fn $name(&self) -> &[$t] {
            unsafe { std::slice::from_raw_parts(self.map.as_ptr().add(self.off[$s as usize]) as *const $t, self.$len) }
        }
    };
}
pub(crate) use sec;

macro_rules! runs {
    ($name:ident, $s:expr) => {
        #[inline]
        pub fn $name(&self, i: usize) -> u32 {
            self.runs($s).get(i)
        }
    };
}

impl Index {
    sec!(name_bits, Sec::NameBits, u64, bits_len);
    sec!(name_base, Sec::NameBase, u32, groups);
    sec!(name_off, Sec::NameOff, u16, u1);
    sec!(names, Sec::Names, u8, names_len);

    #[inline]
    pub fn ent_name(&self, i: usize) -> u32 {
        self.ent_names().get(i)
    }

    #[inline]
    pub fn ent_names(&self) -> Bits<'_> {
        self.bits(Sec::EntName, width(self.u))
    }

    #[inline]
    pub fn kind(&self, i: usize) -> u8 {
        self.bits(Sec::Kind, 4).get(i) as u8
    }

    runs!(size_raw, Sec::Size);
    runs!(mtime, Sec::Mtime);

    sec!(parent_bits, Sec::Parent, u64, n64);
    sec!(parent_rank, Sec::ParentRank, u32, n64);

    #[inline]
    pub fn parent(&self, i: usize) -> u32 {
        self.bits(Sec::ParentMap, width(self.d)).get(self.block_of(i))
    }

    #[inline]
    fn block_of(&self, i: usize) -> usize {
        let (w, r) = (self.parent_bits()[i / 64], self.parent_rank()[i / 64]);
        (r + (w << (63 - i % 64)).count_ones()) as usize - 1
    }

    pub fn blocks(&self, a: usize, b: usize) -> impl Iterator<Item = (std::ops::Range<usize>, u32)> + '_ {
        let (bits, map) = (self.parent_bits(), self.bits(Sec::ParentMap, width(self.d)));
        let (mut i, mut k) = (a, if a < b { self.block_of(a) } else { 0 });
        std::iter::from_fn(move || {
            if i >= b {
                return None;
            }

            let (mut w, mut m) = ((i + 1) / 64, bits.get((i + 1) / 64).map_or(0, |&x| x & (!0 << ((i + 1) % 64))));
            while m == 0 && w * 64 + 64 < b {
                w += 1;
                m = bits[w];
            }
            let end = if m == 0 { b } else { (w * 64 + m.trailing_zeros() as usize).min(b) };
            let run = (i..end, map.get(k));
            (i, k) = (end, k + 1);
            Some(run)
        })
    }

    runs!(dir_entry, Sec::DirEntry);
    runs!(dir_start, Sec::DirStart);
    runs!(dir_len, Sec::DirLen);
    runs!(dir_end, Sec::DirEnd);
    sec!(dir_prior, Sec::DirPrior, i8, d);
    runs!(dir_parent, Sec::DirParent);

    pub fn dir_entries(&self) -> Runs<'_> {
        self.runs(Sec::DirEntry)
    }

    pub fn dir_parents(&self) -> Runs<'_> {
        self.runs(Sec::DirParent)
    }

    #[inline]
    pub fn ents(&self, k: u32) -> impl ExactSizeIterator<Item = u32> + '_ {
        let (off, ne) = (self.bits(Sec::NameEntsOff, width(self.n + 1)), self.bits(Sec::NameEnts, width(self.n)));
        (off.get(k as usize) as usize..off.get(k as usize + 1) as usize).map(move |j| ne.get(j))
    }

    sec!(name_prior, Sec::NamePrior, i8, u);

    sec!(name_info, Sec::NameInfo, NameInfo, u);

    sec!(name_ext, Sec::NameExt, u8, u);

    sec!(ext_table, Sec::Exts, [u8; 16], ext_slots);

    pub fn ext_slot(&self, e: &[u8]) -> u8 {
        let t = self.ext_table();
        (1..255).find(|&i| t[i][0] as usize == e.len() && &t[i][1..1 + e.len()] == e).unwrap_or(255) as u8
    }

    sec!(zones, Sec::Zones, Zone, zones_len);

    pub fn bitmap(&self, b: usize) -> &[u64] {
        &self.name_bits()[b * self.words..][..self.words]
    }

    pub fn class_counts(&self) -> &[u32; CLASSES] {
        &self.counts()[0]
    }

    pub fn double_counts(&self) -> &[u32; CLASSES] {
        &self.counts()[1]
    }

    fn counts(&self) -> &[[u32; CLASSES]; 2] {
        self.counts.get_or_init(|| {
            let count = |b: &[u64]| b.iter().map(|x| x.count_ones()).sum();
            [
                std::array::from_fn(|c| self.bitmap(BM_FIRST + c).iter().zip(self.bitmap(BM_SECOND + c)).map(|(a, b)| (a | b).count_ones()).sum()),
                std::array::from_fn(|c| count(self.bitmap(BM_DOUBLE + c))),
            ]
        })
    }

    pub fn top_prior(&self) -> i8 {
        *self.top_prior.get_or_init(|| self.name_prior().iter().copied().max().unwrap_or(0))
    }

    pub fn by_key(&self) -> &ByKey {
        self.by_key.get_or_init(|| {
            const WANT: usize = 1 << 14;
            let info = self.name_info();
            let mut count = [0usize; 256];
            for x in info {
                count[(x.key as i32 + 128) as usize] += 1;
            }

            let (mut cover, mut n) = (i8::MAX as i32 + 1, 0);
            while cover > i8::MIN as i32 && n + count[(cover - 1 + 128) as usize] <= WANT {
                cover -= 1;
                n += count[(cover + 128) as usize];
            }
            let (mut list, mut short) = (Vec::new(), vec![Vec::new(); 256]);
            for (k, x) in info.iter().enumerate() {
                if x.key as i32 >= cover {
                    list.push((x.key, k as u32));
                }
                if x.len as usize <= SHORT || x.stem as usize <= SHORT {
                    short[x.head as usize].push(k as u32);
                }
            }
            list.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let mut heads = vec![Vec::new(); 256];
            for &(_, k) in &list {
                heads[info[k as usize].head as usize].push(k);
            }
            let counts = (cover..=i8::MAX as i32).map(|k| count[(k + 128) as usize] as u32).collect();
            ByKey { cover, list, counts, heads, short }
        })
    }

    pub fn uname(&self, id: u32) -> Name<'_> {
        let (a, b) = self.name_span(id);
        let s = &self.names()[a..b];
        if s.first() == Some(&PACKED) {
            return unpack(&s[1..]);
        }
        let at = self.off[Sec::Names as usize] + a;
        Name::Raw(s, &self.map[at..])
    }

    #[inline]
    fn name_span(&self, id: u32) -> (usize, usize) {
        let (base, off) = (self.name_base(), self.name_off());
        let o = |k: usize| base[k >> self.name_shift] as usize + off[k] as usize;
        (o(id as usize), o(id as usize + 1))
    }

    pub fn name(&self, i: usize) -> Name<'_> {
        self.uname(self.ent_name(i))
    }

    pub fn size_of(&self, i: usize) -> u64 {
        dec_size(self.size_raw(i))
    }

    #[inline]
    fn bits(&self, s: Sec, w: u32) -> Bits<'_> {
        Bits { p: &self.map[self.off[s as usize]..], w }
    }

    #[inline]
    fn runs(&self, s: Sec) -> Runs<'_> {
        Runs(&self.map[self.off[s as usize]..])
    }

    pub fn sections(&self) -> Vec<(&'static str, usize)> {
        SEC_NAMES.into_iter().zip(section_lens(self.n, self.d, self.u, self.names_len, self.blocks, self.name_shift, self.run_words)).collect()
    }

    pub fn dir_of(&self, entry: u32) -> Option<u32> {
        let k = self.dirs_before(entry);
        (k < self.d && self.dir_entry(k) == entry).then_some(k as u32)
    }

    pub fn dirs_before(&self, e: u32) -> usize {
        let de = self.runs(Sec::DirEntry);
        let r = first_not(self.d.div_ceil(64), |r| de.min(r) < e);
        let at = r.saturating_sub(1) * 64;
        at + first_not((self.d - at).min(64), |j| de.get(at + j) < e)
    }

    pub fn children(&self, d: u32) -> std::ops::Range<usize> {
        let s = self.dir_start(d as usize) as usize;
        s..s + self.dir_len(d as usize) as usize
    }

    pub fn memo_plan(&self) -> &MemoPlan {
        self.plan.get_or_init(|| {
            const CHUNK: u32 = 4096;
            let mut p = MemoPlan { upper: Vec::new(), chunks: Vec::new() };
            let mut big = vec![0u32];
            while let Some(k) = big.pop() {
                let (s, l) = (self.dir_start(k as usize), self.dir_len(k as usize));
                let kids = self.dirs_before(s) as u32..self.dirs_before(s + l) as u32;
                let mut from = kids.end;
                for c in kids {
                    p.upper.push(c);
                    let r = from..self.dirs_before(self.dir_end(c as usize)) as u32;
                    from = r.end;
                    if r.end - r.start > CHUNK {
                        big.push(c)
                    } else if r.start < r.end {
                        p.chunks.push((c, r))
                    }
                }
            }
            p.chunks.sort_by_key(|c| c.1.start);
            p
        })
    }

    pub fn path(&self, i: usize, out: &mut Vec<u8>) {
        out.clear();
        if i == 0 {
            out.push(b'/');
            return;
        }

        let mut chain = [0u32; 256];
        chain[0] = i as u32;
        let (mut k, mut d) = (1, self.parent(i));
        while d != 0 && k < chain.len() {
            chain[k] = self.dir_entry(d as usize);
            k += 1;
            d = self.dir_parent(d as usize);
        }
        for &e in chain[..k].iter().rev() {
            out.push(b'/');
            out.extend_from_slice(&self.name(e as usize));
        }
    }

    pub fn lookup(&self, path: &[u8]) -> Option<u32> {
        let mut e = 0u32;
        for comp in path.split(|&b| b == b'/').filter(|c| !c.is_empty()) {
            let r = self.children(self.dir_of(e)?);
            let (mut lo, mut hi) = (r.start, r.end);
            while lo < hi {
                let mid = (lo + hi) / 2;
                if *self.name(mid) < *comp { lo = mid + 1 } else { hi = mid }
            }
            e = if lo < r.end && *self.name(lo) == *comp { lo as u32 } else { r.clone().find(|&c| self.name(c).eq_ignore_ascii_case(comp))? as u32 };
        }
        Some(e)
    }

    pub fn build(mut ls: Vec<Listing>, event_id: u64, synced_at: u32, home: &[u8]) -> Index {
        ls.par_iter_mut().for_each(|l| {
            let names = &l.names;
            l.ents.sort_unstable_by(|a, b| {
                names[a.name_off as usize..][..a.name_len as usize].cmp(&names[b.name_off as usize..][..b.name_len as usize])
            })
        });
        let max_id = ls.iter().map(|l| l.id).max().unwrap_or(0) as usize;
        let mut by_id = vec![NONE; max_id + 1];
        for (i, l) in ls.iter().enumerate() {
            by_id[l.id as usize] = i as u32;
        }
        let n_max = 1 + ls.iter().map(|l| l.ents.len()).sum::<usize>();
        let names_max: usize = ls.iter().map(|l| l.names.len()).sum();

        let mut name_off = vec![0u32; n_max];
        let mut name_len = vec![0u16; n_max];
        let mut kind = vec![0u8; n_max];
        let mut parent = vec![0u32; n_max];
        let mut enc = vec![0u32; n_max];
        let mut mtime = vec![0u32; n_max];
        let mut names = Vec::with_capacity(names_max);
        kind[0] = KIND_DIR;
        let mut pos = 1usize;
        let mut blocks: Vec<(u32, u32, u32)> = Vec::with_capacity(ls.len());
        let mut stack: Vec<(u32, u32)> = vec![(0, by_id[0])];
        let mut kids: Vec<(u32, u32)> = Vec::new();
        while let Some((e, li)) = stack.pop() {
            let l = std::mem::take(&mut ls[li as usize]);
            let start = pos;
            kids.clear();
            for r in &l.ents {
                name_off[pos] = names.len() as u32;
                name_len[pos] = r.name_len;
                names.extend_from_slice(&l.names[r.name_off as usize..][..r.name_len as usize]);
                kind[pos] = r.kind;
                enc[pos] = enc_size(r.size);
                mtime[pos] = r.mtime;
                parent[pos] = e;
                if let Some(&li) = by_id.get(r.child as usize).filter(|&&li| li != NONE) {
                    kids.push((pos as u32, li));
                }
                pos += 1;
            }
            blocks.push((e, start as u32, (pos - start) as u32));
            stack.extend(kids.iter().rev());
        }
        drop(ls);
        let n = pos;
        blocks.sort_unstable();
        let d = blocks.len();
        let mut dir_of = vec![NONE; n];
        for (k, b) in blocks.iter().enumerate() {
            dir_of[b.0 as usize] = k as u32;
        }
        parent[..n].par_iter_mut().for_each(|p| *p = dir_of[*p as usize]);
        parent[0] = 0;
        drop(dir_of);

        let mut ids: HashMap<&[u8], u32, Fx> = HashMap::with_capacity_and_hasher(n / 3, Fx);
        let mut ent_name = vec![0u32; n];
        let mut uoff = vec![0u32];
        let mut unames: Vec<u8> = Vec::with_capacity(names.len() / 2);
        for i in 0..n {
            let nm = &names[name_off[i] as usize..][..name_len[i] as usize];
            let next = ids.len() as u32;
            let id = *ids.entry(nm).or_insert_with(|| {
                unames.extend_from_slice(nm);
                uoff.push(unames.len() as u32);
                next
            });
            ent_name[i] = id;
        }
        let u = ids.len();
        drop(ids);

        let (mut stored, mut soff) = (Vec::with_capacity(unames.len()), vec![0u32]);
        for k in 0..u {
            let nm = &unames[uoff[k] as usize..uoff[k + 1] as usize];
            if !pack_name(nm, &mut stored) {
                stored.extend_from_slice(nm);
            }
            soff.push(stored.len() as u32);
        }
        let words = u.div_ceil(64);

        let mut by_block = vec![0u64; words * NBITMAPS];
        by_block.par_chunks_mut(NBITMAPS).enumerate().for_each(|(w, out)| {
            for k in w * 64..(w * 64 + 64).min(u) {
                for b in name_bitmaps(&unames[uoff[k] as usize..uoff[k + 1] as usize]) {
                    out[b] |= 1 << (k % 64);
                }
            }
        });
        let mut ubits = vec![0u64; words * NBITMAPS];
        ubits.par_chunks_mut(words).enumerate().for_each(|(b, out)| {
            for (w, o) in out.iter_mut().enumerate() {
                *o = by_block[w * NBITMAPS + b];
            }
        });
        drop(by_block);
        enc.truncate(n);

        let mut ne_off = vec![0u32; u + 1];
        for &id in &ent_name {
            ne_off[id as usize + 1] += 1;
        }
        for k in 0..u {
            ne_off[k + 1] += ne_off[k];
        }
        let mut fill = ne_off[..u].to_vec();
        let mut ne = vec![0u32; n];
        for (i, &id) in ent_name.iter().enumerate() {
            ne[fill[id as usize] as usize] = i as u32;
            fill[id as usize] += 1;
        }
        drop(fill);

        let dir_entry: Vec<u32> = blocks.iter().map(|b| b.0).collect();
        let mut end: Vec<u32> = blocks.iter().map(|b| b.1 + b.2).collect();
        for k in (1..d).rev() {
            let p = parent[dir_entry[k] as usize] as usize;
            end[p] = end[p].max(end[k]);
        }
        let comps: Vec<&[u8]> = home.split(|&b| b == b'/').filter(|c| !c.is_empty()).collect();
        let mut prior = vec![0i8; d];
        let mut depth = vec![0u8; d];

        let mut hm = vec![0u8; d];
        for k in 1..d {
            let e = dir_entry[k] as usize;
            let p = parent[e] as usize;
            let nm = &names[name_off[e] as usize..][..name_len[e] as usize];
            depth[k] = depth[p].saturating_add(1);
            hm[k] = match hm[p] {
                u8::MAX => u8::MAX,
                h if h as usize >= comps.len() => h,
                h if comps[h as usize] == nm => h + 1,
                _ => u8::MAX,
            };
            let entered_home = hm[k] as usize == comps.len() && (hm[p] as usize) < comps.len();
            let adj = prior_adjust(nm, depth[k]) + if entered_home { 15 } else { 0 };
            prior[k] = (prior[p] as i32 + adj).clamp(-100, 60) as i8;
        }
        drop((names, name_off, name_len));

        let (dir_start, dir_len): (Vec<u32>, Vec<u32>) = blocks.iter().map(|b| (b.1, b.2)).unzip();
        drop(blocks);
        let mut starts: Vec<u32> = (0..d as u32).filter(|&k| dir_len[k as usize] > 0).collect();
        starts.sort_unstable_by_key(|&k| dir_start[k as usize]);
        let nblocks = starts.len() + 1;

        let dir_parent: Vec<u32> = dir_entry.iter().map(|&e| parent[e as usize]).collect();
        let runs: [&[u32]; 7] = [&enc, &mtime[..n], &dir_entry, &dir_start, &dir_len, &end, &dir_parent];
        let shapes = runs.map(run_shapes);
        let run_words = shapes.each_ref().map(|s| s.iter().map(|r| r.1 as usize).sum());

        let shift = (0..=5).rev().find(|&s| soff.chunks(1 << s).all(|c| c[c.len() - 1] - c[0] <= u16::MAX as u32)).unwrap();

        let (off, total) = layout(&section_lens(n, d, u, stored.len(), nblocks, shift, run_words));
        let mut m = MmapMut::map_anon(total).expect("anon map");
        let at = |s: Sec| off[s as usize]..;
        let put = |m: &mut [u8], v: &[u8]| m[..v.len()].copy_from_slice(v);
        put(&mut m[at(Sec::NameBits)], as_bytes(&ubits));
        drop(ubits);
        put(&mut m[at(Sec::NameBase)], as_bytes(&soff.iter().step_by(1 << shift).copied().collect::<Vec<_>>()));
        put(&mut m[at(Sec::NameOff)], as_bytes(&soff.iter().enumerate().map(|(k, &o)| (o - soff[k >> shift << shift]) as u16).collect::<Vec<_>>()));
        put(&mut m[at(Sec::Names)], &stored);
        put_bits(&mut m[at(Sec::EntName)], ent_name.iter().copied(), width(u));
        put_bits(&mut m[at(Sec::Kind)], kind[..n].iter().map(|&k| k as u32), 4);
        let mut bits = vec![0u64; n.div_ceil(64)];
        for s in std::iter::once(0).chain(starts.iter().map(|&k| dir_start[k as usize])) {
            bits[s as usize / 64] |= 1 << (s % 64);
        }
        let rank: Vec<u32> = bits.iter().scan(0, |c, w| Some(std::mem::replace(c, *c + w.count_ones()))).collect();
        put(&mut m[at(Sec::Parent)], as_bytes(&bits));
        put(&mut m[at(Sec::ParentRank)], as_bytes(&rank));
        put_bits(&mut m[at(Sec::ParentMap)], std::iter::once(0).chain(starts.iter().copied()), width(d));
        drop((bits, rank, starts));
        put_runs(&mut m[at(Sec::Size)], &enc, &shapes[0]);
        put_runs(&mut m[at(Sec::Mtime)], &mtime[..n], &shapes[1]);
        put(&mut m[at(Sec::Zones)], as_bytes(&zones(&kind[..n], &enc, &mtime[..n])));
        drop((kind, enc, mtime));
        put_runs(&mut m[at(Sec::DirEntry)], &dir_entry, &shapes[2]);
        put_runs(&mut m[at(Sec::DirStart)], &dir_start, &shapes[3]);
        put_runs(&mut m[at(Sec::DirLen)], &dir_len, &shapes[4]);
        put_runs(&mut m[at(Sec::DirEnd)], &end, &shapes[5]);
        put_runs(&mut m[at(Sec::DirParent)], &dir_parent, &shapes[6]);
        put(&mut m[at(Sec::DirPrior)], as_bytes(&prior));
        drop((dir_entry, dir_start, dir_len, end, dir_parent, shapes));

        ne.par_sort_unstable_by_key(|&e| (ent_name[e as usize], std::cmp::Reverse(prior[parent[e as usize] as usize]), e));
        put_bits(&mut m[at(Sec::NameEntsOff)], ne_off.iter().copied(), width(n + 1));
        put_bits(&mut m[at(Sec::NameEnts)], ne.iter().copied(), width(n));
        drop((ne_off, ne));
        let mut name_prior = vec![i8::MIN; u];
        for (i, &k) in ent_name.iter().enumerate().skip(1) {
            name_prior[k as usize] = name_prior[k as usize].max(prior[parent[i] as usize]);
        }
        put(&mut m[at(Sec::NamePrior)], as_bytes(&name_prior));
        let info: Vec<NameInfo> =
            (0..u).into_par_iter().map(|k| NameInfo::of(&unames[uoff[k] as usize..uoff[k + 1] as usize], name_prior[k])).collect();
        put(&mut m[at(Sec::NameInfo)], as_bytes(&info));
        let (table, slots) = ext_slots(&unames, &uoff);
        put(&mut m[at(Sec::NameExt)], &slots);
        put(&mut m[at(Sec::Exts)], as_bytes(&table));
        let f: Vec<u64> =
            [n, d, u, stored.len(), event_id as usize, synced_at as usize, nblocks, shift].into_iter().chain(run_words).map(|v| v as u64).collect();
        m[..HDR].copy_from_slice(&header(MAGIC, &f));
        Index::from_map(m.make_read_only().unwrap()).unwrap()
    }

    fn from_map(map: Mmap) -> Option<Index> {
        let f = fields::<15>(&map, MAGIC)?.map(|v| v as usize);
        let [n, d, u, names_len, event_id, synced_at, blocks, name_shift, ..] = f;
        let run_words: [usize; 7] = f[8..].try_into().unwrap();
        let (off, total) = layout(&section_lens(n, d, u, names_len, blocks, name_shift, run_words));
        let words = u.div_ceil(64);
        if map.len() < total {
            return None;
        }
        Some(Index {
            n,
            d,
            u,
            names_len,
            words,
            bits_len: words * NBITMAPS,
            zones_len: n.div_ceil(ZONE),
            ext_slots: 256,
            n64: n.div_ceil(64),
            blocks,
            name_shift,
            groups: (u + 1).div_ceil(1 << name_shift),
            u1: u + 1,
            run_words,
            event_id: event_id as u64,
            synced_at: synced_at as u32,
            off,
            map,
            plan: std::sync::OnceLock::new(),
            counts: std::sync::OnceLock::new(),
            top_prior: std::sync::OnceLock::new(),
            by_key: std::sync::OnceLock::new(),
        })
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        let mut f = std::fs::File::create(&tmp)?;

        let mut h = self.map[..HDR].to_vec();
        h[8 + 4 * 8..][..16].copy_from_slice(&[self.event_id.to_le_bytes(), (self.synced_at as u64).to_le_bytes()].concat());
        f.write_all(&h)?;
        f.write_all(&self.map[HDR..])?;
        f.sync_data()?;
        std::fs::rename(tmp, path)
    }

    pub fn saved_event_id(path: &Path) -> Option<u64> {
        use std::io::Read;
        let mut h = [0u8; 56];
        std::fs::File::open(path).ok()?.read_exact(&mut h).ok()?;
        fields::<6>(&h, MAGIC).map(|f| f[4])
    }

    pub fn load(path: &Path) -> Option<Index> {
        let f = std::fs::File::open(path).ok()?;
        Index::from_map(unsafe { Mmap::map(&f) }.ok()?)
    }

    pub fn bytes(&self) -> usize {
        self.map.len()
    }

    pub fn prefault(&self) {
        let mut sum = 0u8;
        for s in [Sec::NameBits, Sec::NameBase, Sec::NameOff, Sec::Names, Sec::EntName, Sec::Kind, Sec::Parent, Sec::ParentRank, Sec::ParentMap] {
            let start = self.off[s as usize];
            let end = self.off.get(s as usize + 1).copied().unwrap_or(self.map.len());
            for i in (start..end).step_by(16 * 1024) {
                sum = sum.wrapping_add(unsafe { std::ptr::read_volatile(self.map.as_ptr().add(i)) });
            }
        }
        std::hint::black_box(sum);

        self.class_counts();
        self.top_prior();
        self.by_key();
        self.memo_plan();
    }
}

pub(crate) const HDR: usize = 4096;

pub(crate) fn header(magic: &[u8; 8], fields: &[u64]) -> Vec<u8> {
    let mut h = vec![0u8; HDR];
    h[..8].copy_from_slice(magic);
    for (k, v) in fields.iter().enumerate() {
        h[8 + k * 8..16 + k * 8].copy_from_slice(&v.to_le_bytes());
    }
    h
}

pub(crate) fn fields<const N: usize>(b: &[u8], magic: &[u8; 8]) -> Option<[u64; N]> {
    (b.len() >= 8 + N * 8 && &b[..8] == magic).then(|| std::array::from_fn(|k| u64::from_le_bytes(b[8 + k * 8..16 + k * 8].try_into().unwrap())))
}

pub(crate) fn layout<const N: usize>(lens: &[usize; N]) -> ([usize; N], usize) {
    let mut off = [0usize; N];
    let mut at = HDR;
    for (k, &l) in lens.iter().enumerate() {
        off[k] = at;
        at = (at + l + 63) & !63;
    }
    (off, at)
}

pub(crate) fn as_bytes<T: Copy>(v: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn ext_key(name: &[u8]) -> Option<[u8; 16]> {
    let dot = name.iter().rposition(|&b| b == b'.')?;
    let e = &name[dot + 1..];
    if e.len() > 15 {
        return None;
    }
    let mut key = [0u8; 16];
    key[0] = e.len() as u8;
    for (k, &b) in key[1..].iter_mut().zip(e) {
        *k = crate::query::fold(b);
    }
    Some(key)
}

pub(crate) fn ext_slots(names: &[u8], off: &[u32]) -> (Vec<[u8; 16]>, Vec<u8>) {
    let name = |k: usize| &names[off[k] as usize..off[k + 1] as usize];
    let count = (0..off.len() - 1)
        .into_par_iter()
        .with_min_len(1 << 16)
        .fold(HashMap::<[u8; 16], u32, Fx>::default, |mut m, k| {
            if let Some(e) = ext_key(name(k)) {
                *m.entry(e).or_default() += 1;
            }
            m
        })
        .reduce(HashMap::default, |mut a, b| {
            for (e, n) in b {
                *a.entry(e).or_default() += n;
            }
            a
        });
    let mut common: Vec<([u8; 16], u32)> = count.into_iter().collect();
    common.sort_by(|(a, m), (b, n)| n.cmp(m).then_with(|| a[1..=a[0] as usize].cmp(&b[1..=b[0] as usize])));
    let mut table = vec![[0u8; 16]; 256];
    let mut slot: HashMap<[u8; 16], u8, Fx> = HashMap::default();
    for (i, (e, _)) in common.into_iter().take(254).enumerate() {
        table[i + 1] = e;
        slot.insert(e, i as u8 + 1);
    }
    let slots = (0..off.len() - 1)
        .into_par_iter()
        .map(|k| {
            let n = name(k);
            match ext_key(n) {
                Some(e) => slot.get(&e).copied().unwrap_or(255),
                None => n.contains(&b'.') as u8 * 255,
            }
        })
        .collect();
    (table, slots)
}

impl NameInfo {
    pub fn of(name: &[u8], prior: i8) -> NameInfo {
        let off = (name.len() > 1 && name[0] == b'.') as usize;
        let stem = name.iter().rposition(|&b| b == b'.').filter(|&p| p > off).unwrap_or(name.len());
        let tweak = 10 + if name.ends_with(b".app") { 25 } else { 0 } - if name.first() == Some(&b'.') { 8 } else { 0 };
        NameInfo {
            key: (prior as i32 + tweak - (name.len() as i32).min(80) / 3).max(i8::MIN as i32) as i8,
            len: (name.len() - off).min(255) as u8,
            stem: (stem - off).min(255) as u8,
            head: name.get(off).map_or(0, |&b| crate::query::fold(b)),
        }
    }
}

#[rustfmt::skip]
fn section_lens(n: usize, d: usize, u: usize, names_len: usize, blocks: usize, shift: usize, w: [usize; 7]) -> [usize; NSEC] {
    let n64 = n.div_ceil(64);
    let runs = |len: usize, words: usize| (len.div_ceil(64) + 1 + words) * 8 + 8;
    let z = n.div_ceil(ZONE) * std::mem::size_of::<Zone>();
    [
        u.div_ceil(64) * NBITMAPS * 8, (u + 1).div_ceil(1 << shift) * 4, (u + 1) * 2, names_len, bits_len(n, width(u)), bits_len(n, 4),
        n64 * 8, n64 * 4, bits_len(blocks, width(d)), runs(n, w[0]), runs(n, w[1]), runs(d, w[2]), runs(d, w[3]), runs(d, w[4]),
        runs(d, w[5]), d, runs(d, w[6]), bits_len(u + 1, width(n + 1)), bits_len(n, width(n)), u, u * 4, u, 256 * 16, z,
    ]
}

fn first_not(n: usize, before: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if before(mid) { lo = mid + 1 } else { hi = mid }
    }
    lo
}

fn width(n: usize) -> u32 {
    usize::BITS - n.saturating_sub(1).leading_zeros()
}

#[derive(Clone, Copy)]
pub struct Bits<'a> {
    p: &'a [u8],
    w: u32,
}

impl Bits<'_> {
    #[inline(always)]
    pub fn get(self, i: usize) -> u32 {
        read(self.p, i * self.w as usize, self.w)
    }
}

#[inline(always)]
fn word(p: &[u8], at: usize) -> u64 {
    debug_assert!(at + 8 <= p.len());

    u64::from_le(unsafe { p.as_ptr().add(at).cast::<u64>().read_unaligned() })
}

#[inline(always)]
fn read(p: &[u8], b: usize, w: u32) -> u32 {
    ((word(p, b / 8) >> (b % 8)) & ((1 << w) - 1)) as u32
}

fn bits_len(len: usize, w: u32) -> usize {
    (len * w as usize).div_ceil(8) + 8
}

fn put_bits(out: &mut [u8], v: impl Iterator<Item = u32>, w: u32) {
    for (i, x) in v.enumerate() {
        let b = i * w as usize;
        let o: &mut [u8; 8] = (&mut out[b / 8..][..8]).try_into().unwrap();
        *o = (u64::from_le_bytes(*o) | (x as u64) << (b % 8)).to_le_bytes();
    }
}

#[derive(Clone, Copy)]
pub struct Runs<'a>(&'a [u8]);

impl Runs<'_> {
    #[inline(always)]
    pub fn get(self, i: usize) -> u32 {
        let (lo, s, w) = self.run(i / 64);
        lo + read(self.0, s * 64 + i % 64 * w as usize, w)
    }

    fn min(self, r: usize) -> u32 {
        word(self.0, r * 8) as u32
    }

    #[inline(always)]
    fn run(self, r: usize) -> (u32, usize, u32) {
        let (h, next) = (word(self.0, r * 8), word(self.0, r * 8 + 8));
        (h as u32, (h >> 32) as usize, ((next >> 32) - (h >> 32)) as u32)
    }
}

fn run_shapes(v: &[u32]) -> Vec<(u32, u32)> {
    let shape = |c: &[u32]| c.iter().fold((u32::MAX, 0), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    v.chunks(64).map(shape).map(|(lo, hi)| (lo, u32::BITS - (hi - lo).leading_zeros())).collect()
}

fn put_runs(out: &mut [u8], v: &[u32], shapes: &[(u32, u32)]) {
    let mut at = shapes.len() + 1;
    for (r, (c, &(lo, w))) in v.chunks(64).zip(shapes).enumerate() {
        out[r * 8..][..8].copy_from_slice(&(lo as u64 | (at as u64) << 32).to_le_bytes());
        put_bits(&mut out[at * 8..], c.iter().map(|&x| x - lo), w);
        at += w as usize;
    }
    out[shapes.len() * 8 + 4..][..4].copy_from_slice(&(at as u32).to_le_bytes());
}

pub const ZONE: usize = 1024;

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct Zone {
    pub max_size: u32,
    pub min_mtime: u32,
    pub max_mtime: u32,

    pub kinds: u32,
}

pub(crate) fn zones(kind: &[u8], size: &[u32], mtime: &[u32]) -> Vec<Zone> {
    (0..kind.len().div_ceil(ZONE))
        .map(|z| {
            let r = z * ZONE..((z + 1) * ZONE).min(kind.len());
            Zone {
                max_size: size[r.clone()].iter().copied().max().unwrap_or(0),
                min_mtime: mtime[r.clone()].iter().copied().min().unwrap_or(0),
                max_mtime: mtime[r.clone()].iter().copied().max().unwrap_or(0),
                kinds: kind[r].iter().fold(0, |m, &k| m | 1 << (k & 3)),
            }
        })
        .collect()
}

pub fn enc_size(s: u64) -> u32 {
    if s < 1 << 31 { s as u32 } else { (1 << 31) | (s >> 21).min((1 << 31) - 1) as u32 }
}

pub fn dec_size(v: u32) -> u64 {
    if v & (1 << 31) == 0 { v as u64 } else { ((v & !(1 << 31)) as u64) << 21 }
}

#[derive(Clone, Copy, Default)]
pub struct Fx;
pub struct FxH(u64);
impl std::hash::BuildHasher for Fx {
    type Hasher = FxH;
    fn build_hasher(&self) -> FxH {
        FxH(0)
    }
}
impl std::hash::Hasher for FxH {
    fn write(&mut self, bytes: &[u8]) {
        for c in bytes.chunks(8) {
            let mut w = [0u8; 8];
            w[..c.len()].copy_from_slice(c);
            self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(w)).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

pub fn char_mask(s: &[u8]) -> u64 {
    s.iter().fold(0, |m, &b| m | char_bit(b))
}

pub fn name_mask(s: &[u8]) -> u64 {
    let off = (s.len() > 1 && s[0] == b'.') as usize;
    let starts = s.get(off).into_iter().chain(s.windows(2).filter(|w| w[0] == b' ').map(|w| &w[1]));
    starts.fold(char_mask(s), |m, &b| m | start_bit(b))
}

#[inline]
pub fn start_bit(b: u8) -> u64 {
    1 << (CLASSES + start_hash(b))
}

#[inline]
pub fn start_hash(b: u8) -> usize {
    (b.to_ascii_lowercase() % 23) as usize
}

pub const CLASSES: usize = 41;

pub const BM_FIRST: usize = 0;
pub const BM_SECOND: usize = CLASSES;

pub const BM_START: usize = 2 * CLASSES;

pub const BM_DOT: usize = BM_START + 23;
pub const BM_APP: usize = BM_DOT + 1;

pub const BM_DOUBLE: usize = BM_APP + 1;
pub const NBITMAPS: usize = BM_DOUBLE + CLASSES;

fn name_bitmaps(s: &[u8]) -> impl Iterator<Item = usize> {
    let (a, b) = s.split_at(s.len() / 2);
    let class = |m: u64, base: usize| (0..CLASSES).filter(move |c| m & (1 << c) != 0).map(move |c| base + c);
    let starts = name_mask(s) >> CLASSES;
    class(char_mask(a), BM_FIRST)
        .chain(class(char_mask(b), BM_SECOND))
        .chain((0..23).filter(move |h| starts & (1 << h) != 0).map(|h| BM_START + h))
        .chain(s.first().is_some_and(|&c| c == b'.').then_some(BM_DOT))
        .chain(s.ends_with(b".app").then_some(BM_APP))
        .chain(class(doubled(s), BM_DOUBLE))
}

pub fn doubled(s: &[u8]) -> u64 {
    let (mut once, mut twice) = (0, 0);
    for &b in s {
        twice |= once & char_bit(b);
        once |= char_bit(b);
    }
    twice
}

#[inline]
pub fn char_bit(b: u8) -> u64 {
    match b {
        b'a'..=b'z' => 1 << (b - b'a'),
        b'A'..=b'Z' => 1 << (b - b'A'),
        b'0'..=b'9' => 1 << (26 + b - b'0'),
        b'.' => 1 << 36,
        b'-' | b'_' => 1 << 37,
        b' ' => 1 << 38,
        0x80.. => 1 << 39,
        _ => 1 << 40,
    }
}

#[rustfmt::skip]
const BUNDLE_EXTS: &[&[u8]] = &[
    b".framework", b".bundle", b".plugin", b".appex", b".kext", b".xpc", b".lproj", b".xcassets", b".photoslibrary",
    b".musiclibrary", b".tvlibrary", b".imovielibrary", b".dSYM", b".xcarchive", b".sdk", b".platform",
];

fn prior_adjust(name: &[u8], depth: u8) -> i32 {
    if depth == 1 {
        return match name {
            b"Users" => 0,
            b"Applications" => 10,
            b"Volumes" => -10,
            b"Library" => -25,
            b"System" => -40,
            b"opt" => -25,
            _ => -35,
        };
    }
    if name == b"Applications" {
        return 30;
    }
    if name.ends_with(b".app") {
        return -25;
    }
    if BUNDLE_EXTS.iter().any(|x| name.len() > x.len() && name[name.len() - x.len()..].eq_ignore_ascii_case(x)) {
        return -20;
    }
    if name.first() == Some(&b'.') {
        return -25;
    }
    match name {
        b"Library" => -20,
        b"Caches" | b"caches" | b"cache" | b"Cache" | b"Logs" | b"DerivedData" | b"CoreSimulator" => -20,
        b"node_modules" | b"__pycache__" | b"site-packages" | b"Pods" | b"venv" | b"bower_components" => -30,
        b"target" | b"build" | b"dist" | b"out" | b"vendor" | b"deps" | b"tmp" | b"temp" => -12,
        b"folders" | b"Containers" | b"Group Containers" => -10,
        b"Application Support" => -5,
        _ => 0,
    }
}
