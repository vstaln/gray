//! Resident file index (fff-search) shared by `find` and `grep`.
//!
//! One `fff_search::FilePicker` per canonical search root, created lazily on
//! first use — the harness opens sessions in arbitrary directories, so an
//! eager index would pay a 500k-file scan for a cwd that never gets searched.
//! The picker owns a background filesystem watcher; subsequent calls hit warm
//! memory instead of respawning `fd`/`rg` per query.
//!
//! Frecency databases live under `<frecency_root>/<hash-of-dir>`; the global
//! pool roots them at `gray_home()/fff-frecency` so ranking survives restarts.
//!
//! Every `indexed_*` entry point returns `Option`: `None` means "index
//! unavailable, fall back to the spawn/walk lanes" — the tool contract never
//! depends on the index being present. Keeping those lanes authoritative is
//! why the decline rules are conservative: fff paginates *before* a caller
//! can filter, so a pattern the index cannot answer exactly is declined
//! rather than answered approximately.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fff_search::file_picker::{FilePicker, FilePickerOptions, FuzzySearchOptions};
use fff_search::frecency::FrecencyTracker;
use fff_search::grep::{GrepMode, GrepSearchOptions};
use fff_search::{
    Constraint, ContentCacheBudget, FFFMode, FFFQuery, FuzzyQuery, PaginationArgs,
    SharedFilePicker, SharedFrecency,
};
use tokio_util::sync::CancellationToken;

use crate::grep_builtin::{Found, Hit};

/// First-use scan budget: an index that cannot finish scanning a tree in this
/// window defers to the fd/rg fallback rather than blocking the tool call.
const SCAN_WAIT: Duration = Duration::from_secs(10);

/// Resident roots kept warm at once. Each one is a full index, a content
/// cache, and a watcher thread, so a session hopping across trees evicts the
/// oldest instead of growing without bound; a tree beyond the cap simply uses
/// the fd/rg lanes, which is what it cost before the index existed.
const MAX_RESIDENT_ROOTS: usize = 4;

/// Content cache cap per index. fff auto-sizes this to 512 MB for a repo
/// under 10k files — a fine default for a single-purpose picker, far too much
/// for an agent that can hold `MAX_RESIDENT_ROOTS` of them at once.
const CACHE_FILES: usize = 4096;
const CACHE_BYTES: u64 = 64 * 1024 * 1024;

/// A warm-probe (`warm_picker`) never blocks: it polls the pool's `resident`
/// mutex this many times before answering "not warm". A probe that blocks is
/// how one wedged build freezes every search in the process — measured live on
/// Windows CI, where fff construction stalled inside its own dependencies and
/// `warm_picker`'s blocking `lock()` hung the caller for the job's remaining
/// 48 minutes. `PROBE_POLLS * PROBE_POLL_INTERVAL` ≈ 50ms, cheaper than one
/// fd spawn; past that the fd/rg fallback lane answers, as it always may.
const PROBE_POLLS: usize = 10;
const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// A caller joining an already-running build waits this long for the publish
/// before declining to the fallback lane. Builds are seconds; the wait exists
/// only to dodge one redundant fd spawn in the common two-searches-in-a-row
/// case, so it must stay far below what a user would call slow.
const BUILD_JOIN_WAIT: Duration = Duration::from_secs(3);
const JOIN_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Per-root picker pool. Cheap to construct; indexes are built on demand.
pub struct SearchPool {
    frecency_root: PathBuf,
    resident: Mutex<Resident>,
    /// Roots with a build in flight. The build itself runs with NO pool lock
    /// held (fff construction can stall inside its own dependencies — LMDB
    /// writer lock, git status, watcher init — and a stall under the pool
    /// lock used to block every probe and search behind it), so this set is
    /// the only dedup between a background builder and a synchronous caller
    /// asking for the same root.
    builds: Mutex<HashSet<PathBuf>>,
    /// Calls served by this pool's indexes — the only way to tell the
    /// identical-output lanes apart, since both lanes return the same bytes.
    hits: AtomicUsize,
}

/// Live indexes plus their insertion order: `HashMap` has no order and
/// eviction needs one.
#[derive(Default)]
struct Resident {
    pickers: HashMap<PathBuf, SharedFilePicker>,
    order: VecDeque<PathBuf>,
}

impl SearchPool {
    pub fn new(frecency_root: PathBuf) -> Self {
        Self {
            frecency_root,
            resident: Mutex::new(Resident::default()),
            builds: Mutex::new(HashSet::new()),
            hits: AtomicUsize::new(0),
        }
    }

    /// Tool calls served by the index rather than a fallback lane.
    pub fn lane_hits(&self) -> usize {
        self.hits.load(Ordering::Relaxed)
    }

    /// Number of live indexes (one per canonical dir requested so far).
    /// Bounded like every other pool read: a contended mutex reads as zero,
    /// never as a wait.
    pub fn len(&self) -> usize {
        self.resident
            .try_lock()
            .map(|r| r.pickers.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Shared picker for `dir`, indexed lazily on first request. `None` when
    /// `dir` is not a readable directory, sits outside any git worktree (fff
    /// drops dotfiles on non-git roots — the fd/rg lanes keep those), a build
    /// is already in flight and the join window lapses, or the index cannot
    /// start.
    pub fn picker(&self, dir: &Path) -> Option<SharedFilePicker> {
        let dir = dir.canonicalize().ok()?;
        if !dir.is_dir() {
            return None;
        }
        // Cheap pre-check so non-git dirs never pay for an index; the
        // authoritative `has_git_repo` gate stays post-scan in `indexed_*`.
        if !dir.ancestors().any(|p| p.join(".git").exists()) {
            return None;
        }
        // Fast path: already resident. The lock is held only to clone a
        // handle — microseconds, and still taken through the bounded probe so
        // even a wedged publisher cannot block a caller here.
        if let Some(p) = self.probe_resident(&dir) {
            return Some(p);
        }
        // Dedup: an in-flight build for the same root is joined, not
        // duplicated. Bounded — past the window the fd/rg lane answers.
        {
            let mut builds = self.builds.lock().ok()?;
            if !builds.insert(dir.clone()) {
                drop(builds);
                return self.join_in_flight_build(&dir);
            }
        }
        // The build runs with NO pool lock held. Everything below — frecency
        // open, fff construction, watcher init — reaches unbounded waits
        // inside fff and its deps; holding `resident` across them is what
        // turned one wedged build into a process-wide search hang on Windows.
        let built = self.build_picker(&dir);
        if let Ok(mut builds) = self.builds.lock() {
            builds.remove(&dir);
        }
        let shared = built?;
        // Publish under a short lock; eviction is bounded bookkeeping.
        let mut res = self.resident.lock().ok()?;
        while res.order.len() >= MAX_RESIDENT_ROOTS {
            if let Some(evicted) = res.order.pop_front() {
                // Dropping the shared picker drops the FilePicker with it,
                // and fff's `BackgroundWatcher::drop` stops the watch.
                res.pickers.remove(&evicted);
            }
        }
        res.pickers.insert(dir.clone(), shared.clone());
        res.order.push_back(dir);
        Some(shared)
    }

    /// The unbounded part of index construction, deliberately lock-free.
    fn build_picker(&self, dir: &Path) -> Option<SharedFilePicker> {
        let shared = SharedFilePicker::default();
        let frecency = SharedFrecency::default();
        // Best-effort frecency persistence: a read-only or exhausted home dir
        // must not cost the index.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        dir.hash(&mut hasher);
        let db_dir = self.frecency_root.join(format!("{:016x}", hasher.finish()));
        if std::fs::create_dir_all(&db_dir).is_ok()
            && let Ok(tracker) = FrecencyTracker::open(&db_dir)
        {
            let _ = frecency.init(tracker);
        }

        FilePicker::new_with_shared_state(
            shared.clone(),
            frecency,
            FilePickerOptions {
                base_path: dir.to_string_lossy().to_string(),
                mode: FFFMode::Ai,
                enable_content_indexing: true,
                cache_budget: ContentCacheBudget::from_overrides(CACHE_FILES, CACHE_BYTES, 0),
                ..Default::default()
            },
        )
        .ok()?;
        Some(shared)
    }

    /// One bounded non-blocking read of the resident map.
    fn probe_resident(&self, dir: &Path) -> Option<SharedFilePicker> {
        for _ in 0..PROBE_POLLS {
            match self.resident.try_lock() {
                Ok(res) => return res.pickers.get(dir).cloned(),
                Err(_) => std::thread::sleep(PROBE_POLL_INTERVAL),
            }
        }
        None
    }

    /// Wait (bounded) for an in-flight build of `dir` to publish. `None` once
    /// the join window lapses — the fd/rg lane answers instead.
    fn join_in_flight_build(&self, dir: &Path) -> Option<SharedFilePicker> {
        let deadline = std::time::Instant::now() + BUILD_JOIN_WAIT;
        loop {
            if let Some(p) = self.probe_resident(dir) {
                return Some(p);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(JOIN_POLL_INTERVAL);
        }
    }

    /// The index for `dir` if this process already built one. Never builds,
    /// never blocks: a caller deciding whether to *pay* for an index must be
    /// able to ask "is it already warm?" without triggering the scan that
    /// answers the question — and without hanging on a contended or wedged
    /// builder (the Windows CI hang was exactly this probe blocking on the
    /// build lock, 48 minutes, no timeout reachable). A brief probe of the
    /// resident map; `None` means the fd/rg lane is the cheaper answer now.
    pub fn warm_picker(&self, dir: &Path) -> Option<SharedFilePicker> {
        let dir = dir.canonicalize().ok()?;
        self.probe_resident(&dir)
    }

    /// Start building the index for `dir` in the background, without waiting
    /// for it: the *next* search in this process finds it warm.
    ///
    /// This is the whole reason the search command is not "index first": a
    /// fresh process holding no index would otherwise pay a full scan
    /// (measured: 1.4s on a 20k-file repo) to answer a question `fd` answers
    /// in 20ms. So the first search goes to `fd`, and the index earns its keep
    /// from the second one on. No-op once the root is resident.
    pub fn warm_in_background(self: &Arc<Self>, dir: &Path) {
        if self.warm_picker(dir).is_some() {
            return;
        }
        let dir = dir.to_path_buf();
        // A build already in flight will publish on its own; spawning a second
        // builder would race it to the same resident slot.
        if self
            .builds
            .lock()
            .map(|builds| builds.contains(&dir))
            .unwrap_or(true)
        {
            return;
        }
        let pool = self.clone();
        std::thread::spawn(move || {
            // `picker` publishes the index and returns without waiting for the
            // scan; the scan and the watcher are background threads from here.
            let _ = pool.picker(&dir);
        });
    }

    /// `find` lane: glob `pattern` over the resident index → relative paths,
    /// frecency-ranked, capped at `limit`. `None` = caller falls back to
    /// fd/manual walk.
    pub fn indexed_glob(&self, dir: &Path, pattern: &str, limit: usize) -> Option<Vec<String>> {
        // Absolute globs are matched against full paths by fd --full-path;
        // the index only knows relative paths — defer to the fallback.
        if pattern.starts_with('/') {
            return None;
        }
        // A slash pattern is anchored at a depth fff's glob cannot express:
        // its `Constraint::Glob` compiles with `literal_separator = false`, so
        // `crates/gray-tools/src/*.rs` also matches every file below `src/`
        // (69 hits where fd reports 23 in this very repo). Pagination happens
        // inside fff, before any post-filter, so serving it would either
        // over-report or silently drop matches — decline, let fd --full-path
        // answer exactly.
        if pattern.contains('/') {
            return None;
        }
        let shared = self.picker(dir)?;
        if !shared.wait_for_scan(SCAN_WAIT) {
            return None;
        }
        let guard = shared.read().ok()?;
        let picker = guard.as_ref()?;
        // Contract gate: fff's walker skips dotfiles on non-git roots and only
        // honors .gitignore inside repos; fd --hidden does neither. Serve the
        // index only where semantics agree — everything else falls back.
        if !picker.has_git_repo() {
            return None;
        }

        // fd --glob semantics: a bare pattern is a basename match at any
        // depth. `**/` gets us there (globset treats a leading `**/` as
        // zero-or-more directories) and leaves the pattern itself slash-free.
        let pat = if pattern == "**" {
            pattern.to_string()
        } else {
            format!("**/{pattern}")
        };
        // Two globset details decide whether this lane is exact or a guess:
        //  - `literal_separator` defaults to FALSE, so `*` also eats `/` and
        //    `*_test*` would match `a_test_dir/file.rs`. fd compares the
        //    pattern against the basename only. We re-filter every hit.
        //  - an uncompilable pattern (`*.{rs`) makes fff match nothing, while
        //    fd reports the parse error — decline so fd produces that error.
        let matcher = globset::GlobBuilder::new(&pat)
            .literal_separator(true)
            .build()
            .ok()?
            .compile_matcher();
        // fff paginates before we filter, so ask for headroom: an over-matched
        // page must not leave us short of the caller's limit.
        let fetch = limit.saturating_mul(2).min(4096);
        let options = FuzzySearchOptions {
            pagination: PaginationArgs {
                offset: 0,
                limit: fetch,
            },
            ..Default::default()
        };
        let mut out: Vec<String> = picker
            .glob(&pat, options)
            .items
            .iter()
            .map(|f| f.relative_path(picker))
            .filter(|rel| matcher.is_match(rel))
            .collect();
        // fd returns matching directories with a trailing slash too. fff's
        // directory search is fuzzy-text only (glob constraints don't apply),
        // so match indexed dirs against the same glob ourselves.
        out.extend(
            picker
                .get_dirs()
                .iter()
                .map(|d| d.relative_path(picker))
                // Indexed dir paths carry a trailing slash ("sub/"); the
                // glob sees bare names.
                .filter(|rel| matcher.is_match(rel.trim_end_matches('/')))
                .map(|rel| format!("{}/", rel.trim_end_matches('/'))),
        );
        out.truncate(limit);
        self.hits.fetch_add(1, Ordering::Relaxed);
        Some(out)
    }

    /// `grep` lane: content search over the resident index → same `Found`
    /// shape `grep_builtin::search` returns, so `format_matches` renders
    /// identically regardless of lane. `None` = caller falls back to
    /// rg/builtin.
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
        if cancel.is_cancelled() {
            return None;
        }
        let shared = self.picker(dir)?;
        if !shared.wait_for_scan(SCAN_WAIT) {
            return None;
        }
        let guard = shared.read().ok()?;
        let picker = guard.as_ref()?;
        // Same non-git contract gate as indexed_glob.
        if !picker.has_git_repo() {
            return None;
        }

        // fff 0.10.x only offers smart_case, which can't express an explicit
        // insensitive search — the `(?i)` workaround would push literals off
        // the SIMD path. Decline: rg serves ignoreCase natively until we can
        // bump to 0.11's `case_mode`.
        if ignore_case {
            return None;
        }
        // rg -g '!*.rs' is an exclusion; as Constraint::Glob the `!` is a
        // literal filename char (Not(Glob) is a separate variant). Decline
        // rather than translate — the contract stays rg-verified.
        if glob.is_some_and(|g| g.starts_with('!')) {
            return None;
        }
        // A glob with a slash is depth-anchored, and fff compiles globs with
        // `literal_separator = false` (`a/*.rs` would also match `a/b/c.rs`).
        // 0.10.6 happens to decline those queries already; declining here
        // keeps it that way across a version bump. A slash-free glob needs no
        // filter: globset matches it against the basename, exactly like rg.
        if glob.is_some_and(|g| g.contains('/')) {
            return None;
        }
        let (mode, text) = if literal {
            (GrepMode::PlainText, pattern.to_string())
        } else {
            (GrepMode::Regex, pattern.to_string())
        };
        let query = FFFQuery {
            raw_query: pattern,
            constraints: glob.map(|g| vec![Constraint::Glob(g)]).unwrap_or_default(),
            fuzzy_query: FuzzyQuery::Text(&text),
            location: None,
        };

        // fff search is synchronous CPU work; forward ctx.cancel into its
        // abort signal so a Ctrl-C mid-search still stops promptly.
        let abort = Arc::new(AtomicBool::new(false));
        {
            let (abort, cancel) = (abort.clone(), cancel.clone());
            std::thread::spawn(move || {
                loop {
                    if abort.load(Ordering::Relaxed) || cancel.is_cancelled() {
                        abort.store(true, Ordering::Relaxed);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
        }
        let result = picker.grep(
            &query,
            &GrepSearchOptions {
                mode,
                smart_case: false,
                page_limit: limit,
                before_context: context,
                after_context: context,
                abort_signal: Some(abort.clone()),
                ..Default::default()
            },
        );
        abort.store(true, Ordering::Relaxed); // releases the forwarder thread

        // An invalid regex falls back to literal inside fff and reports the
        // error here; gray's contract is to fail the call — let rg produce
        // the canonical "invalid pattern" error instead.
        if result.regex_fallback_error.is_some() {
            return None;
        }
        // Constrained query that found nothing → fff retries the raw pattern
        // as literal text with glob constraints DROPPED (issue #756). rg's
        // contract is zero hits; a fallback match outside the glob is wrong.
        if result.literal_fallback {
            return None;
        }

        // page_limit is a batch-level cap: the result may overshoot it.
        // Truncate to the caller's limit and flag the limit notice.
        let overshot = result.matches.len() > limit;
        let mut hits: Vec<Hit> = Vec::with_capacity(result.matches.len());
        for m in result.matches.iter().take(limit) {
            let Some(file) = result.files.get(m.file_index) else {
                continue;
            };
            // Paths feed format_matches, which relativizes against the raw
            // (uncanonicalized) search dir — join onto `dir`, not base_path.
            let path = dir
                .join(file.relative_path(picker))
                .to_string_lossy()
                .to_string();
            let before = m.context_before.len() as u64;
            for (i, ctx_line) in m.context_before.iter().enumerate() {
                hits.push((
                    path.clone(),
                    (m.line_number - before + i as u64) as usize,
                    Some(ctx_line.clone()),
                    false,
                ));
            }
            hits.push((
                path.clone(),
                m.line_number as usize,
                Some(m.line_content.clone()),
                true,
            ));
            for (i, ctx_line) in m.context_after.iter().enumerate() {
                hits.push((
                    path.clone(),
                    (m.line_number + 1 + i as u64) as usize,
                    Some(ctx_line.clone()),
                    false,
                ));
            }
        }
        self.hits.fetch_add(1, Ordering::Relaxed);
        Some(Found {
            hits,
            // page_limit reached mid-scan leaves the next file offset
            // pointing past what we searched — more matches may exist.
            limit_reached: overshot || result.next_file_offset != 0,
        })
    }
}

/// Process-wide pool; frecency DBs under `gray_home()/fff-frecency`.
pub fn global_pool() -> &'static Arc<SearchPool> {
    static POOL: std::sync::OnceLock<Arc<SearchPool>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let root = gray_core::paths::gray_home()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fff-frecency");
        Arc::new(SearchPool::new(root))
    })
}

#[path = "search_index_tests.rs"]
#[cfg(test)]
mod tests;
