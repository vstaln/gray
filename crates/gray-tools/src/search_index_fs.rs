use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use fsearch::{Case, Engine, Grep, GrepMode, Query};
use tokio_util::sync::CancellationToken;

use crate::grep_builtin::Found;
use crate::index_filter::{IgnoreChain, Lane};

const FETCH_HEADROOM: usize = 4;
const FETCH_MIN: usize = 64;

const FETCH_MAX: usize = 4096;

const GREP_FILE_CAP: usize = 8192;

pub struct FsearchPool {
    opts: fsearch::Options,
    engine: OnceLock<Option<Engine>>,
    hits: AtomicUsize,
}

impl Default for FsearchPool {
    fn default() -> Self {
        Self::new()
    }
}

impl FsearchPool {
    pub fn new() -> Self {
        let home =
            std::env::var_os("HOME").map_or_else(String::new, |h| h.to_string_lossy().into_owned());
        Self::with_options(fsearch::Options {
            dir: gray_core::paths::gray_home()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("fsearch"),
            home,

            root: Some(PathBuf::from("/")),
            skip: None,
        })
    }

    pub fn with_options(opts: fsearch::Options) -> Self {
        Self {
            opts,
            engine: OnceLock::new(),
            hits: AtomicUsize::new(0),
        }
    }

    pub fn lane_hits(&self) -> usize {
        self.hits.load(Ordering::Relaxed)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn engine(&self) -> Option<&Engine> {
        self.engine
            .get_or_init(|| {
                fsearch::set_log_sink(Some(std::sync::Arc::new(|line: &str| {
                    log::info!(target: "fsearch", "{line}");
                })));
                Engine::start(self.opts.clone()).ok()
            })
            .as_ref()
    }

    pub fn warm_picker(&self, dir: &Path) -> Option<PathBuf> {
        let e = self.engine()?;
        if !e.status().ready {
            return None;
        }
        let cdir = dir.canonicalize().ok()?;
        if !cdir.is_dir() || !e.covers(cdir.as_os_str().as_bytes()) {
            return None;
        }
        Some(cdir)
    }

    pub fn warm_in_background(self: &Arc<Self>, _dir: &Path) {
        let _ = self.engine();
    }

    pub fn indexed_glob(&self, dir: &Path, pattern: &str, limit: usize) -> Option<Vec<String>> {
        let cdir = self.warm_picker(dir)?;
        let e = self.engine()?;

        if pattern.starts_with('/') || pattern.contains('/') {
            return None;
        }

        globset::GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .ok()?;

        let name_re =
            regex::bytes::Regex::new(&format!("^(?:{})", glob_to_regex(pattern)?)).ok()?;
        let fetch = limit
            .saturating_mul(FETCH_HEADROOM)
            .saturating_add(FETCH_MIN)
            .min(FETCH_MAX);
        let q = Query {
            scope: Some(cdir.as_os_str().as_bytes().to_vec()),
            name_re: Some(name_re),
            limit: fetch,
            ..parse_defaults()
        };
        let hits = e.search(&q).ok()?;

        if hits.len() >= fetch {
            return None;
        }
        let mut chain = IgnoreChain::new(&cdir, Lane::Find);
        let mut out = Vec::with_capacity(hits.len());
        for h in &hits {
            let is_dir = h.kind & 3 == fsearch::walk::KIND_DIR;
            if chain.is_ignored(&h.path, is_dir) {
                continue;
            }
            let Ok(rel) = h.path.strip_prefix(&cdir) else {
                continue;
            };
            let mut s = rel.to_string_lossy().into_owned();
            if is_dir {
                s.push('/');
            }
            out.push(s);
        }
        out.truncate(limit);
        self.hits.fetch_add(1, Ordering::Relaxed);
        Some(out)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn indexed_grep(
        &self,
        dir: &Path,
        pattern: &str,
        glob: Option<&str>,
        ignore_case: bool,
        literal: bool,
        limit: usize,
        context: usize,
        cancel: &CancellationToken,
    ) -> Option<Found> {
        if cancel.is_cancelled() || context > 0 {
            return None;
        }
        let cdir = self.warm_picker(dir)?;
        let e = self.engine()?;

        let st = e.status();
        if st.content_pending > 0 || st.content_docs == 0 {
            return None;
        }

        if glob.is_some_and(|g| g.starts_with('!') || g.contains('/') || g.contains('{')) {
            return None;
        }
        let mut g = Grep::with_case(
            pattern,
            if literal {
                GrepMode::Literal
            } else {
                GrepMode::Regex
            },
            if ignore_case {
                Case::Insensitive
            } else {
                Case::Sensitive
            },
        )
        .ok()?;

        g.max_per_file = limit.max(1);
        let mut q = Query {
            scope: Some(cdir.as_os_str().as_bytes().to_vec()),
            limit: GREP_FILE_CAP,
            ..parse_defaults()
        };
        if let Some(glob) = glob {
            globset::GlobBuilder::new(glob)
                .literal_separator(true)
                .build()
                .ok()?;
            q.name_re =
                Some(regex::bytes::Regex::new(&format!("^(?:{})", glob_to_regex(glob)?)).ok()?);
        }
        let (res, _indexed) = e.grep(&q, &g).ok()?;

        if !res.complete {
            return None;
        }
        let mut chain = IgnoreChain::new(&cdir, Lane::Grep);
        let mut hits = Vec::new();
        let mut limit_reached = res.files.len() >= GREP_FILE_CAP;
        'files: for f in &res.files {
            let p = Path::new(std::ffi::OsStr::from_bytes(&f.path));
            if chain.is_ignored(p, false) {
                continue;
            }
            let Ok(rel) = p.strip_prefix(&cdir) else {
                continue;
            };

            let path = dir.join(rel).to_string_lossy().into_owned();
            for (ln, text) in &f.lines {
                if hits.len() >= limit {
                    limit_reached = true;
                    break 'files;
                }
                hits.push((path.clone(), *ln, Some(text.clone()), true));
            }
        }
        self.hits.fetch_add(1, Ordering::Relaxed);
        Some(Found {
            hits,
            limit_reached,
        })
    }
}

fn parse_defaults() -> Query {
    Query {
        size: (0, u64::MAX),
        mtime: (0, u32::MAX),
        ..Default::default()
    }
}

fn glob_to_regex(pat: &str) -> Option<String> {
    fn inner(
        chars: &mut std::iter::Peekable<std::str::Chars>,
        out: &mut String,
        in_alt: bool,
    ) -> Option<()> {
        while let Some(c) = chars.next() {
            match c {
                '*' => out.push_str(".*"),
                '?' => out.push('.'),
                '[' => {
                    out.push('[');
                    if chars.next_if_eq(&'!').is_some() {
                        out.push('^');
                    }
                    if chars.next_if_eq(&']').is_some() {
                        out.push_str("\\]");
                    }
                    let mut closed = false;
                    while let Some(c) = chars.next() {
                        match c {
                            ']' => {
                                out.push(']');
                                closed = true;
                                break;
                            }
                            '[' if chars.peek() == Some(&':') => {
                                out.push('[');
                                for c in chars.by_ref() {
                                    out.push(c);
                                    if c == ']' {
                                        break;
                                    }
                                }
                            }
                            '\\' => {
                                out.push('\\');
                                out.push(chars.next()?);
                            }
                            c => out.push(c),
                        }
                    }
                    if !closed {
                        return None;
                    }
                }
                '{' => {
                    out.push_str("(?:");
                    inner(chars, out, true)?;
                    out.push(')');
                }
                ',' if in_alt => out.push('|'),
                '}' if in_alt => return Some(()),
                '\\' => {
                    out.push_str("\\\\");
                    let n = chars.next()?;
                    if n.is_ascii_punctuation() {
                        out.push('\\');
                    }
                    out.push(n);
                }
                c if c.is_ascii_punctuation() => {
                    out.push('\\');
                    out.push(c);
                }
                c => out.push(c),
            }
        }
        (!in_alt).then_some(())
    }
    let mut out = String::new();
    inner(&mut pat.chars().peekable(), &mut out, false)?;
    Some(out)
}

pub fn global_fs_pool() -> &'static Arc<FsearchPool> {
    static POOL: std::sync::OnceLock<Arc<FsearchPool>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| Arc::new(FsearchPool::new()))
}
