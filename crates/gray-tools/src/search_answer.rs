use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::search_plan::{self, GrepPlan, RgPlan};

pub(crate) enum Answer {
    #[allow(dead_code)]
    Exec {
        argv: Vec<String>,
        files: Vec<String>,
        mode: &'static str,
    },
    Print {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        code: i32,
    },
}

enum Op {
    Indexed {
        scope: PathBuf,
        prefix: String,
    },
    #[allow(dead_code)]
    Pass(String),
}

fn walk_key(
    order: &mut HashMap<PathBuf, Option<HashMap<OsString, usize>>>,
    scope: &Path,
    rel: &Path,
) -> Option<Vec<usize>> {
    let mut dir = scope.to_path_buf();
    let mut key = Vec::new();
    for c in rel.components() {
        let name = c.as_os_str();
        let listing = order
            .entry(dir.clone())
            .or_insert_with(|| readdir_order(&dir));
        key.push(*listing.as_ref()?.get(name)?);
        dir.push(name);
    }
    Some(key)
}

fn readdir_order(dir: &Path) -> Option<HashMap<OsString, usize>> {
    let names: Vec<OsString> = std::fs::read_dir(dir)
        .ok()?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<Result<_, _>>()
        .ok()?;
    if names.len() <= FTS_INODE_SORT_ENTRIES {
        return Some(names.into_iter().enumerate().map(|(i, n)| (n, i)).collect());
    }
    use std::os::unix::fs::MetadataExt as _;
    let mut by_ino: Vec<(u64, OsString)> = names
        .into_iter()
        .map(|n| {
            let ino = std::fs::metadata(dir.join(&n))
                .map(|m| m.ino())
                .unwrap_or(0);
            (ino, n)
        })
        .collect();
    by_ino.sort();
    Some(
        by_ino
            .into_iter()
            .enumerate()
            .map(|(i, (_, n))| (n, i))
            .collect(),
    )
}

const FTS_INODE_SORT_ENTRIES: usize = 10_000;

fn walked(plan: &GrepPlan, rel: &Path) -> bool {
    let mut comps = rel.components().peekable();
    while let Some(c) = comps.next() {
        let name = c.as_os_str().as_bytes();
        if comps.peek().is_none() {
            return plan.walk.keeps_file(name);
        }
        if plan.walk.skips_dir(name) {
            return false;
        }
    }
    false
}

struct Block {
    rel: PathBuf,

    key: Vec<usize>,

    display: String,

    abs: PathBuf,

    fm: Option<fsearch::FileMatches>,

    binary: bool,

    matched: bool,

    err: Option<String>,
}

type LastPrinted = Option<(usize, Vec<u8>, usize)>;

fn print_answer(
    engine: &fsearch::Engine,
    plan: &GrepPlan,
    ops: &[Op],
    order: &mut HashMap<PathBuf, Option<HashMap<OsString, usize>>>,
) -> Result<Option<Answer>, &'static str> {
    let Some(mut g) = plan.fsearch_grep() else {
        if std::env::var_os("SERVE_DBG").is_some() {
            eprintln!("bail: fsearch_grep");
        }
        return Ok(None);
    };
    let mut blocks: Vec<(usize, Block)> = Vec::new();
    let mut walked_files: Vec<(usize, PathBuf)> = Vec::new();
    for (oi, op) in ops.iter().enumerate() {
        let Op::Indexed { scope, prefix } = op else {
            if std::env::var_os("SERVE_DBG").is_some() {
                eprintln!("bail: op not indexed");
            }
            return Ok(None);
        };
        let scope_bytes = scope.as_os_str().as_bytes();

        g.skip = 0;
        loop {
            let q = fsearch::Query {
                scope: Some(scope_bytes.to_vec()),
                limit: usize::MAX,
                size: (0, u64::MAX),
                mtime: (0, u32::MAX),
                ..Default::default()
            };
            if std::env::var_os("SERVE_DBG").is_some() {
                eprintln!("serve[engine.grep {scope:?}]", scope = scope);
            }
            let (res, _idx) = engine.grep(&q, &g).map_err(|_| "index grep failed")?;
            if std::env::var_os("SERVE_DBG").is_some() {
                eprintln!("serve[grepped: {} files]", res.files.len());
            }
            if !res.complete {
                return Ok(None);
            }
            let next = res.next;
            for fm in res.files {
                let path = Path::new(OsStr::from_bytes(&fm.path));
                let Ok(rel) = path.strip_prefix(scope) else {
                    continue;
                };
                if !walked(plan, rel) {
                    continue;
                }
                let Some(key) = walk_key(order, scope, rel) else {
                    if std::env::var_os("SERVE_DBG").is_some() {
                        eprintln!("bail: no walk_key {rel:?}");
                    }
                    return Ok(None);
                };
                let Some(name) = rel.to_str() else {
                    if std::env::var_os("SERVE_DBG").is_some() {
                        eprintln!("bail: nonutf8 {rel:?}");
                    }
                    return Ok(None);
                };
                blocks.push((
                    oi,
                    Block {
                        key,
                        rel: rel.to_path_buf(),
                        display: format!("{prefix}{name}"),
                        abs: path.to_path_buf(),
                        fm: Some(fm),
                        binary: false,
                        matched: true,
                        err: None,
                    },
                ));
            }
            match next {
                Some(n) => g.skip = n,
                None => break,
            }
        }

        if std::env::var_os("SERVE_DBG").is_some() {
            eprintln!("serve[changed]");
        }
        if std::env::var_os("SERVE_DBG").is_some() {
            eprintln!("serve[unheld_files]");
        }
        let mut extra: BTreeSet<Vec<u8>> = engine
            .unheld_files(scope_bytes)
            .ok_or("content index not holding scope")?
            .into_iter()
            .collect();
        if std::env::var_os("SERVE_DBG").is_some() {
            eprintln!("serve[names]");
        }
        let known = engine
            .search(&fsearch::Query {
                scope: Some(scope_bytes.to_vec()),
                name_re: None,
                size: (0, u64::MAX),
                mtime: (0, u32::MAX),
                limit: 1 << 24,
                ..Default::default()
            })
            .map_err(|_| "index list failed")?
            .into_iter()
            .map(|f| f.path.as_os_str().as_bytes().to_vec())
            .collect::<std::collections::HashSet<Vec<u8>>>();
        if std::env::var_os("SERVE_DBG").is_some() {
            eprintln!("serve[walk]");
        }
        let mut stack = vec![scope.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                if std::env::var_os("SERVE_DBG").is_some() {
                    eprintln!("bail: readdir {d:?}");
                }
                return Ok(None);
            };
            for e in rd.flatten() {
                let p = e.path();
                let bytes = p.as_os_str().as_bytes();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if plan.count || !known.contains(bytes) {
                        stack.push(p);
                    }
                    continue;
                }

                if !ft.is_file() {
                    continue;
                }
                if plan.count
                    && let Ok(rel) = p.strip_prefix(scope)
                    && walked(plan, rel)
                {
                    walked_files.push((oi, rel.to_path_buf()));
                }
                if !known.contains(bytes) {
                    extra.insert(bytes.to_vec());
                }
            }
        }
        if std::env::var_os("SERVE_DBG").is_some() {
            eprintln!("serve[extra {} files]", extra.len());
        }

        for p in extra {
            let path = Path::new(OsStr::from_bytes(&p));
            let Ok(rel) = path.strip_prefix(scope) else {
                continue;
            };
            if !walked(plan, rel) || !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
                continue;
            }
            if blocks.iter().any(|(bo, b)| {
                *bo == oi && b.rel.as_os_str().as_bytes() == rel.as_os_str().as_bytes()
            }) {
                continue;
            }
            let Some(key) = walk_key(order, scope, rel) else {
                if std::env::var_os("SERVE_DBG").is_some() {
                    eprintln!("bail: walk_key2 {rel:?}");
                }
                return Ok(None);
            };
            let Some(name) = rel.to_str() else {
                if std::env::var_os("SERVE_DBG").is_some() {
                    eprintln!("bail: nonutf8 {rel:?}");
                }
                return Ok(None);
            };
            let display = format!("{prefix}{name}");
            let block = match read_block(&g, path, plan) {
                ReadBlock::File {
                    fm,
                    binary,
                    matched,
                } => Block {
                    key,
                    rel: rel.to_path_buf(),
                    display,
                    abs: path.to_path_buf(),
                    fm,
                    binary,
                    matched,
                    err: None,
                },
                ReadBlock::Err(e) => Block {
                    key,
                    rel: rel.to_path_buf(),
                    display,
                    abs: path.to_path_buf(),
                    fm: None,
                    binary: false,
                    matched: false,
                    err: Some(e),
                },
                ReadBlock::Bail => {
                    if std::env::var_os("SERVE_DBG").is_some() {
                        eprintln!("bail: read_block {path:?}");
                    }
                    return Ok(None);
                }
            };
            blocks.push((oi, block));
        }
    }

    blocks.retain(|(_, b)| b.fm.is_some() || (b.binary && b.matched) || b.err.is_some());
    blocks.sort_by(|(ao, a), (bo, b)| (ao, &a.key).cmp(&(bo, &b.key)));

    blocks.retain(|(_, b)| {
        !std::fs::symlink_metadata(&b.abs).is_ok_and(|m| m.file_type().is_symlink())
    });

    if plan.count && !plan.files_only {
        return emit_count(plan, ops, blocks, walked_files, order);
    }
    if std::env::var_os("SERVE_DBG").is_some() {
        eprintln!("serve[emit]");
    }
    emit(plan, blocks)
}

pub(crate) fn grep_answer(engine: &fsearch::Engine, argv: &[String], cwd: &Path) -> Option<Answer> {
    let plan = search_plan::grep_plan(argv).ok()?;
    if !plan.recursive
        || !plan.answerable()
        || !plan.all_needled
        || plan.needles.is_empty()
        || plan.operands.is_empty()
    {
        return None;
    }
    let st = engine.status();
    if !st.ready || st.content_pending > 0 || st.content_docs == 0 {
        return None;
    }
    let mut ops = Vec::with_capacity(plan.operands.len());
    for raw in &plan.operands {
        let joined = cwd.join(raw);
        let scope = (!raw.is_empty() && joined.is_dir())
            .then(|| joined.canonicalize().ok())
            .flatten()
            .filter(|s| {
                engine.covers(s.as_os_str().as_bytes())
                    && !s
                        .file_name()
                        .is_some_and(|n| plan.walk.skips_dir(n.as_bytes()))
            })?;
        let trimmed = raw.trim_end_matches('/');
        let prefix = if raw.starts_with('/') && trimmed.is_empty() {
            "/".to_string()
        } else {
            format!("{trimmed}/")
        };
        ops.push(Op::Indexed { scope, prefix });
    }
    let mut order = HashMap::new();
    match print_answer(engine, &plan, &ops, &mut order) {
        Ok(Some(a @ Answer::Print { .. })) => Some(a),
        _ => None,
    }
}

pub(crate) fn rg_answer(engine: &fsearch::Engine, argv: &[String], cwd: &Path) -> Option<Answer> {
    let plan = search_plan::rg_plan(argv).ok()?;
    if !plan.answerable()
        || !plan.all_needled
        || plan.needles.is_empty()
        || plan.operands.is_empty()
    {
        return None;
    }
    let st = engine.status();
    if !st.ready || st.content_pending > 0 || st.content_docs == 0 {
        return None;
    }
    let mut ops = Vec::with_capacity(plan.operands.len());
    for raw in &plan.operands {
        let joined = cwd.join(raw);
        let scope = (!raw.is_empty() && raw != "-" && joined.is_dir())
            .then(|| joined.canonicalize().ok())
            .flatten()
            .filter(|s| engine.covers(s.as_os_str().as_bytes()))?;
        let trimmed = raw.trim_end_matches('/');
        let prefix = if raw.starts_with('/') && trimmed.is_empty() {
            "/".to_string()
        } else {
            format!("{trimmed}/")
        };
        ops.push(Op::Indexed { scope, prefix });
    }
    match rg_print_answer(engine, &plan, &ops) {
        Ok(Some(a @ Answer::Print { .. })) => Some(a),
        _ => None,
    }
}

fn emit_count(
    plan: &GrepPlan,
    ops: &[Op],
    blocks: Vec<(usize, Block)>,
    walked: Vec<(usize, PathBuf)>,
    order: &mut HashMap<PathBuf, Option<HashMap<OsString, usize>>>,
) -> Result<Option<Answer>, &'static str> {
    let show_name = plan.with_name.unwrap_or(true);
    let mut by_rel: HashMap<(usize, Vec<u8>), &Block> = HashMap::new();
    for (oi, b) in &blocks {
        by_rel.insert((*oi, b.rel.as_os_str().as_bytes().to_vec()), b);
    }
    let mut rows: Vec<(usize, Vec<usize>, PathBuf, String)> = Vec::new();
    for (oi, rel) in walked {
        let Op::Indexed { scope, prefix } = &ops[oi] else {
            return Ok(None);
        };
        let Some(key) = walk_key(order, scope, &rel) else {
            return Ok(None);
        };
        let Some(name) = rel.to_str() else {
            return Ok(None);
        };
        rows.push((oi, key, rel.clone(), format!("{prefix}{name}")));
    }
    rows.sort();
    let mut out = Vec::<u8>::new();
    let mut err = String::new();
    let mut errored = false;
    let mut any = false;
    for (oi, _, rel, display) in &rows {
        let n = match by_rel.get(&(*oi, rel.as_os_str().as_bytes().to_vec())) {
            None => 0,
            Some(b) => {
                if let Some(e) = &b.err {
                    if !plan.no_messages {
                        use std::fmt::Write as _;
                        let _ = writeln!(err, "grep: {}: {}", display, e);
                    }
                    errored = true;
                    continue;
                }
                match &b.fm {
                    None => 0,
                    Some(fm) => count_lines(fm, plan)?,
                }
            }
        };
        any |= n > 0;
        if show_name {
            out.extend_from_slice(display.as_bytes());
            out.push(b':');
        }
        out.extend_from_slice(n.to_string().as_bytes());
        out.push(b'\n');
    }
    if plan.quiet {
        out.clear();
    }
    let code = if errored {
        2
    } else if any {
        0
    } else {
        1
    };
    Ok(Some(Answer::Print {
        stdout: out,
        stderr: err.into_bytes(),
        code,
    }))
}

fn count_lines(fm: &fsearch::FileMatches, plan: &GrepPlan) -> Result<usize, &'static str> {
    if !plan.word {
        return Ok(fm.lines.len());
    }
    let mut n = 0;
    for (ln, hit) in fm.lines.iter().zip(fm.hits.iter()) {
        if ln.1.contains('\u{FFFD}') {
            return Err("lossy text");
        }
        n += hit.ranges.iter().any(|r| word_keep(&ln.1, *r)) as usize;
    }
    Ok(n)
}

enum ReadBlock {
    File {
        fm: Option<fsearch::FileMatches>,
        binary: bool,
        matched: bool,
    },
    Err(String),

    Bail,
}

fn read_block(g: &fsearch::Grep, path: &Path, plan: &GrepPlan) -> ReadBlock {
    let buf = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return ReadBlock::Err(e.to_string()),
    };
    if buf.contains(&0) {
        if std::env::var_os("SERVE_DBG").is_some() {
            eprintln!("serve[binary {:?} match={}]", path, g.matches_buf(&buf));
        }
        if plan.ignore_binary {
            return ReadBlock::File {
                fm: None,
                binary: true,
                matched: false,
            };
        }
        if g.matches_buf(&buf) {
            if plan.only_matching
                || plan.before > 0
                || plan.after > 0
                || plan.line_match
                || plan.word
                || plan.count
            {
                return ReadBlock::Bail;
            }
            return ReadBlock::File {
                fm: None,
                binary: true,
                matched: true,
            };
        }
        return ReadBlock::File {
            fm: None,
            binary: true,
            matched: false,
        };
    }
    let res = fsearch::grep_file(g, path.as_os_str().as_bytes());
    let fm = res.files.into_iter().next();
    let matched = fm.is_some();
    ReadBlock::File {
        fm,
        binary: false,
        matched,
    }
}

fn emit(plan: &GrepPlan, mut blocks: Vec<(usize, Block)>) -> Result<Option<Answer>, &'static str> {
    if plan.ignore_binary {
        blocks.retain(|(_, b)| !b.binary);
    }
    let show_name = plan.with_name.unwrap_or(true);
    let mut out = Vec::<u8>::new();
    let mut err = String::new();
    let mut errored = false;
    let mut matched = false;

    let mut printed = false;
    let mut last: LastPrinted = None;
    for (oi, b) in &blocks {
        if let Some(e) = &b.err {
            if !plan.no_messages {
                use std::fmt::Write as _;
                let _ = writeln!(err, "grep: {}: {}", b.display, e);
            }
            errored = true;
            continue;
        }
        if b.binary && !b.matched {
            continue;
        }
        match emit_block(
            &mut out,
            &mut err,
            plan,
            b,
            *oi,
            show_name,
            &mut printed,
            &mut last,
        )? {
            Some(m) => {
                matched |= m;

                if plan.quiet && m {
                    break;
                }
            }
            None => {
                if std::env::var_os("SERVE_DBG").is_some() {
                    eprintln!("bail: emit_block {b:?}", b = b.display);
                }
                return Ok(None);
            }
        }
    }
    let code = if errored {
        2
    } else if matched {
        0
    } else {
        1
    };
    if plan.quiet {
        out.clear();
    }
    Ok(Some(Answer::Print {
        stdout: out,
        stderr: err.into_bytes(),
        code,
    }))
}

#[allow(clippy::too_many_arguments)]
fn emit_block(
    out: &mut Vec<u8>,
    err: &mut String,
    plan: &GrepPlan,
    b: &Block,
    oi: usize,
    show_name: bool,
    printed: &mut bool,
    last: &mut LastPrinted,
) -> Result<Option<bool>, &'static str> {
    if !b.matched {
        return Ok(Some(false));
    }
    if b.binary {
        if plan.files_only {
            out.extend_from_slice(b.display.as_bytes());
            out.push(b'\n');
        } else if !plan.quiet {
            err.push_str(&format!("grep: {}: binary file matches\n", b.display));
        }
        *printed = true;
        return Ok(Some(true));
    }
    let Some(fm) = &b.fm else {
        return Ok(Some(b.matched));
    };

    if plan.files_only {
        if plan.word && !word_match(fm, b)? {
            return Ok(Some(false));
        }
        out.extend_from_slice(b.display.as_bytes());
        out.push(b'\n');
        *printed = true;
        return Ok(Some(true));
    }

    let ctx = plan.before.max(plan.after);
    let raw = std::fs::read(&b.abs).map_err(|_| "file vanished mid-search")?;
    let lines: Vec<&[u8]> = raw.split(|&c| c == b'\n').collect();
    let line = |ln: usize| -> Result<&[u8], &'static str> {
        lines.get(ln.wrapping_sub(1)).copied().ok_or("line moved")
    };
    let mut rows: std::collections::BTreeMap<usize, Row> = std::collections::BTreeMap::new();
    let mut pending: Vec<usize> = Vec::new();
    let mut windows: Vec<(usize, usize)> = Vec::new();
    let mut any_match = false;
    for ((ln, _text), hit) in fm.lines.iter().zip(fm.hits.iter()) {
        let ranges: Vec<(usize, usize)> = if plan.word || plan.only_matching {
            let raw_l = line(*ln)?;
            let sh = String::from_utf8_lossy(raw_l);
            let sh = sh.trim_end();
            if sh.contains('\u{FFFD}') {
                return Ok(None);
            }
            if plan.word {
                hit.ranges
                    .iter()
                    .cloned()
                    .filter(|r| word_keep(sh, *r))
                    .collect()
            } else {
                hit.ranges.clone()
            }
        } else {
            hit.ranges.clone()
        };
        if plan.word && ranges.is_empty() {
            pending.push(*ln);
            continue;
        }
        any_match = true;
        rows.insert(
            *ln,
            Row {
                is_match: true,
                ranges,
            },
        );
        if ctx > 0 && !plan.only_matching {
            windows.push((ln.saturating_sub(plan.before), ln + plan.after));
            for (k, _c) in hit.before.iter().enumerate() {
                let n = ln.saturating_sub(hit.before.len() - k);
                rows.entry(n).or_insert(Row {
                    is_match: false,
                    ranges: Vec::new(),
                });
            }
            for (k, _c) in hit.after.iter().enumerate() {
                let n = ln + 1 + k;
                rows.entry(n).or_insert(Row {
                    is_match: false,
                    ranges: Vec::new(),
                });
            }
        }
    }
    if !any_match {
        return Ok(Some(false));
    }
    for ln in pending {
        if windows.iter().any(|(lo, hi)| ln >= *lo && ln <= *hi) {
            rows.insert(
                ln,
                Row {
                    is_match: false,
                    ranges: Vec::new(),
                },
            );
        }
    }
    let mut emitted_any = false;
    for (ln, row) in &rows {
        let adjacent = last.as_ref().is_some_and(|(loi, lp, ll)| {
            *loi == oi && lp == b.abs.as_os_str().as_bytes() && *ln <= ll + 1
        });
        if ctx > 0 && !plan.only_matching && *printed && !adjacent {
            out.extend_from_slice(b"--\n");
        }
        let raw_l = line(*ln)?;
        if row.is_match {
            if plan.only_matching {
                let sh = String::from_utf8_lossy(raw_l);
                let sh = sh.trim_end();
                if sh.contains('\u{FFFD}') {
                    return Ok(None);
                }
                for r in &row.ranges {
                    let Some(seg) = sh.get(r.0..r.1) else {
                        return Ok(None);
                    };
                    print_line(
                        out,
                        b,
                        plan,
                        show_name,
                        plan.line_number,
                        *ln,
                        seg.as_bytes(),
                    );
                }
            } else {
                print_line(out, b, plan, show_name, plan.line_number, *ln, raw_l);
            }
        } else {
            print_ctx(out, b, plan, show_name, *ln, raw_l);
        }
        *printed = true;
        emitted_any = true;
        *last = Some((oi, b.abs.as_os_str().as_bytes().to_vec(), *ln));
    }
    Ok(Some(emitted_any))
}

struct Row {
    is_match: bool,
    ranges: Vec<(usize, usize)>,
}

fn word_match(fm: &fsearch::FileMatches, b: &Block) -> Result<bool, &'static str> {
    for ((ln, text), hit) in fm.lines.iter().zip(fm.hits.iter()) {
        if text.contains('\u{FFFD}') {
            return Ok(false);
        }
        let full = ensure_full(b, *ln, text)?;
        let text = full.as_deref().unwrap_or(text.as_str());
        if hit.ranges.iter().any(|r| word_keep(text, *r)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_full(b: &Block, ln: usize, text: &str) -> Result<Option<String>, &'static str> {
    if text.len() < 400 {
        return Ok(None);
    }
    let buf = std::fs::read(&b.abs).map_err(|_| "file vanished mid-search")?;
    let line = buf.split(|&c| c == b'\n').nth(ln - 1).ok_or("line moved")?;
    Ok(Some(String::from_utf8_lossy(line).trim_end().to_string()))
}

fn word_keep(text: &str, r: (usize, usize)) -> bool {
    let bs = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let left = r.0 == 0 || !word(bs[r.0 - 1]);
    let right = r.1 >= bs.len() || !word(bs[r.1]);
    left && right
}

fn print_line(
    out: &mut Vec<u8>,
    b: &Block,
    _plan: &GrepPlan,
    show_name: bool,
    numbered: bool,
    ln: usize,
    text: &[u8],
) {
    if show_name {
        out.extend_from_slice(b.display.as_bytes());
        out.push(b':');
    }
    if numbered {
        out.extend_from_slice(ln.to_string().as_bytes());
        out.push(b':');
    }
    out.extend_from_slice(text);
    out.push(b'\n');
}

fn print_ctx(
    out: &mut Vec<u8>,
    b: &Block,
    plan: &GrepPlan,
    show_name: bool,
    ln: usize,
    text: &[u8],
) {
    if show_name {
        out.extend_from_slice(b.display.as_bytes());
        out.push(b'-');
    }
    if plan.line_number {
        out.extend_from_slice(ln.to_string().as_bytes());
        out.push(b'-');
    }
    out.extend_from_slice(text);
    out.push(b'\n');
}

enum WalkItem {
    File(PathBuf),
    Err(PathBuf, String),
}

fn rg_walk(scope: &Path, plan: &RgPlan) -> Result<Vec<WalkItem>, &'static str> {
    let hidden = plan.hidden.unwrap_or(false);
    let mut wb = ignore::WalkBuilder::new(scope);
    wb.hidden(!hidden)
        .follow_links(false)
        .max_depth(plan.max_depth)
        .parents(!plan.no_ignore && !plan.no_ignore_parent)
        .ignore(!plan.no_ignore && !plan.no_ignore_dot)
        .git_ignore(!plan.no_ignore && !plan.no_ignore_vcs)
        .git_exclude(!plan.no_ignore && !plan.no_ignore_vcs && !plan.no_ignore_exclude)
        .git_global(!plan.no_ignore && !plan.no_ignore_global)
        .require_git(plan.require_git);
    if !plan.no_ignore && !plan.no_ignore_dot {
        wb.add_custom_ignore_filename(".rgignore");
    }
    for f in &plan.ignore_files {
        let _ = wb.add_ignore(f);
    }
    if !plan.globs.is_empty() {
        let mut ob = ignore::overrides::OverrideBuilder::new(scope);
        for g in &plan.globs {
            ob.add(g).map_err(|_| "bad glob")?;
        }
        wb.overrides(ob.build().map_err(|_| "bad glob")?);
    }
    let mut items = Vec::new();
    for e in wb.build() {
        match e {
            Ok(dent) => {
                let Some(ft) = dent.file_type() else { continue };
                if !ft.is_file() || ft.is_symlink() {
                    continue;
                }
                if let Some(max) = plan.max_filesize
                    && dent.metadata().map(|m| m.len()).unwrap_or(0) > max
                {
                    continue;
                }
                items.push(WalkItem::File(dent.into_path()));
            }
            Err(e) => {
                let (p, msg) = walk_err(&e);
                items.push(WalkItem::Err(p.unwrap_or_else(|| scope.to_path_buf()), msg));
            }
        }
    }
    Ok(items)
}

fn walk_err(e: &ignore::Error) -> (Option<PathBuf>, String) {
    use ignore::Error;
    let mut cur = e;
    let mut path = None;
    loop {
        match cur {
            Error::WithPath { path: p, err } => {
                if path.is_none() {
                    path = Some(p.clone());
                }
                cur = err;
            }
            Error::WithDepth { err, .. } | Error::WithLineNumber { err, .. } => cur = err,
            Error::Io(ioe) => return (path, ioe.to_string()),
            _ => return (path, cur.to_string()),
        }
    }
}

fn rg_display(prefix: &str, name: &str, plan: &RgPlan) -> String {
    let d = format!("{prefix}{name}");
    match &plan.path_separator {
        Some(sep) if sep != "/" => d.replace('/', sep),
        _ => d,
    }
}

struct RgBlock {
    display: String,
    abs: PathBuf,
    fm: Option<fsearch::FileMatches>,
    err: Option<String>,
}

fn rg_print_answer(
    engine: &fsearch::Engine,
    plan: &RgPlan,
    ops: &[Op],
) -> Result<Option<Answer>, &'static str> {
    let Some(mut g) = plan.fsearch_grep() else {
        return Ok(None);
    };
    let mut blocks: Vec<(usize, RgBlock)> = Vec::new();
    for (oi, op) in ops.iter().enumerate() {
        let Op::Indexed { scope, prefix } = op else {
            return Ok(None);
        };
        let scope_bytes = scope.as_os_str().as_bytes();

        let mut held: HashMap<Vec<u8>, fsearch::FileMatches> = HashMap::new();
        g.skip = 0;
        loop {
            let q = fsearch::Query {
                scope: Some(scope_bytes.to_vec()),
                limit: usize::MAX,
                size: (0, u64::MAX),
                mtime: (0, u32::MAX),
                ..Default::default()
            };
            let (res, _idx) = engine.grep(&q, &g).map_err(|_| "index grep failed")?;
            if !res.complete {
                return Ok(None);
            }
            let next = res.next;
            for fm in res.files {
                held.insert(fm.path.clone(), fm);
            }
            match next {
                Some(n) => g.skip = n,
                None => break,
            }
        }

        let extra: BTreeSet<Vec<u8>> = engine
            .unheld_files(scope_bytes)
            .ok_or("content index not holding scope")?
            .into_iter()
            .collect();
        let known: std::collections::HashSet<Vec<u8>> = engine
            .search(&fsearch::Query {
                scope: Some(scope_bytes.to_vec()),
                name_re: None,
                size: (0, u64::MAX),
                mtime: (0, u32::MAX),
                limit: 1 << 24,
                ..Default::default()
            })
            .map_err(|_| "index list failed")?
            .into_iter()
            .map(|f| f.path.as_os_str().as_bytes().to_vec())
            .collect();
        for item in rg_walk(scope, plan)? {
            match item {
                WalkItem::Err(p, msg) => {
                    let display = p
                        .strip_prefix(scope)
                        .ok()
                        .and_then(|r| r.to_str())
                        .map(|n| rg_display(prefix, n, plan))
                        .unwrap_or_else(|| p.display().to_string());
                    blocks.push((
                        oi,
                        RgBlock {
                            display,
                            abs: p,
                            fm: None,
                            err: Some(msg),
                        },
                    ));
                }
                WalkItem::File(path) => {
                    let bytes = path.as_os_str().as_bytes();
                    if let Some(fm) = held.remove(bytes) {
                        let buf = std::fs::read(&path).map_err(|_| "file vanished")?;
                        if buf.contains(&0) {
                            return Ok(None);
                        }
                        let rel = path.strip_prefix(scope).map_err(|_| "walk mismatch")?;
                        let name = rel.to_str().ok_or("path is not UTF-8")?;
                        blocks.push((
                            oi,
                            RgBlock {
                                display: rg_display(prefix, name, plan),
                                abs: path,
                                fm: Some(fm),
                                err: None,
                            },
                        ));
                    } else if extra.contains(bytes) || !known.contains(bytes) {
                        let rel = path.strip_prefix(scope).map_err(|_| "walk mismatch")?;
                        let name = rel.to_str().ok_or("path is not UTF-8")?;
                        let display = rg_display(prefix, name, plan);
                        let buf = match std::fs::read(&path) {
                            Ok(b) => b,
                            Err(e) => {
                                blocks.push((
                                    oi,
                                    RgBlock {
                                        display,
                                        abs: path,
                                        fm: None,
                                        err: Some(e.to_string()),
                                    },
                                ));
                                continue;
                            }
                        };
                        if buf.contains(&0) {
                            if g.matches_buf(&buf) {
                                return Ok(None);
                            }
                            continue;
                        }
                        let fm = fsearch::grep_file(&g, path.as_os_str().as_bytes())
                            .files
                            .into_iter()
                            .next();
                        if let Some(fm) = fm {
                            blocks.push((
                                oi,
                                RgBlock {
                                    display,
                                    abs: path,
                                    fm: Some(fm),
                                    err: None,
                                },
                            ));
                        }
                    }
                }
            }
        }
    }
    rg_emit(plan, blocks)
}

fn rg_emit(plan: &RgPlan, blocks: Vec<(usize, RgBlock)>) -> Result<Option<Answer>, &'static str> {
    let show_name = plan.with_name.unwrap_or(true);
    let numbered = plan.numbered();
    let column = plan.column || plan.vimgrep;
    let heading = plan.heading.unwrap_or(false) && show_name && !plan.null_sep;

    let file_blank = plan.heading.unwrap_or(false) && plan.null_sep;
    let sep: u8 = if plan.null_sep { 0 } else { b':' };
    let mut out = Vec::<u8>::new();
    let mut err = String::new();
    let mut errored = false;
    let mut matched = false;
    let mut printed = false;
    let mut last: Option<(usize, Vec<u8>, usize)> = None;
    let mut headed: Option<Vec<u8>> = None;
    for (oi, b) in &blocks {
        if let Some(e) = &b.err {
            if !plan.no_messages {
                use std::fmt::Write as _;
                let _ = writeln!(err, "rg: {}: {}", b.display, e);
            }
            errored = true;
            continue;
        }
        let Some(fm) = &b.fm else { continue };
        match plan.mode {
            search_plan::RgMode::Files => {
                out.extend_from_slice(b.display.as_bytes());
                out.push(if plan.null_sep { 0 } else { b'\n' });
                matched = true;
            }
            search_plan::RgMode::Count => {
                let n = rg_count(fm, plan.only_matching);
                if show_name {
                    out.extend_from_slice(b.display.as_bytes());
                    out.push(sep);
                }
                out.extend_from_slice(n.to_string().as_bytes());
                out.push(b'\n');
                matched |= n > 0;
            }
            search_plan::RgMode::Lines => {
                match rg_emit_block(
                    &mut out,
                    plan,
                    b,
                    *oi,
                    show_name,
                    numbered,
                    column,
                    heading,
                    file_blank,
                    &mut printed,
                    &mut last,
                    &mut headed,
                    fm,
                )? {
                    Some(()) => matched = true,
                    None => return Ok(None),
                }
            }
        }
        if plan.quiet && matched {
            break;
        }
    }
    let code = if plan.quiet {
        if matched {
            0
        } else if errored {
            2
        } else {
            1
        }
    } else if errored {
        2
    } else if matched {
        0
    } else {
        1
    };
    if plan.quiet {
        out.clear();
    }
    Ok(Some(Answer::Print {
        stdout: out,
        stderr: err.into_bytes(),
        code,
    }))
}

fn rg_count(fm: &fsearch::FileMatches, matches: bool) -> usize {
    if !matches {
        return fm.lines.len();
    }
    fm.hits.iter().map(|h| h.ranges.len()).sum()
}

struct RgRow {
    is_match: bool,
    ranges: Vec<(usize, usize)>,
}

#[allow(clippy::too_many_arguments)]
fn rg_emit_block(
    out: &mut Vec<u8>,
    plan: &RgPlan,
    b: &RgBlock,
    oi: usize,
    show_name: bool,
    numbered: bool,
    column: bool,
    heading: bool,
    file_blank: bool,
    printed: &mut bool,
    last: &mut Option<(usize, Vec<u8>, usize)>,
    headed: &mut Option<Vec<u8>>,
    fm: &fsearch::FileMatches,
) -> Result<Option<()>, &'static str> {
    let ctx = plan.before.max(plan.after);
    let bytes = b.abs.as_os_str().as_bytes().to_vec();
    let raw = std::fs::read(&b.abs).map_err(|_| "file vanished mid-search")?;
    let lines: Vec<&[u8]> = raw.split(|&c| c == b'\n').collect();
    let mut rows: std::collections::BTreeMap<usize, RgRow> = std::collections::BTreeMap::new();
    for ((ln, _), hit) in fm.lines.iter().zip(fm.hits.iter()) {
        rows.insert(
            *ln,
            RgRow {
                is_match: true,
                ranges: hit.ranges.clone(),
            },
        );
        if ctx > 0 {
            for k in 0..hit.before.len() {
                let n = ln.saturating_sub(hit.before.len() - k);
                rows.entry(n).or_insert(RgRow {
                    is_match: false,
                    ranges: Vec::new(),
                });
            }
            for k in 0..hit.after.len() {
                rows.entry(ln + 1 + k).or_insert(RgRow {
                    is_match: false,
                    ranges: Vec::new(),
                });
            }
        }
    }

    if (heading || file_blank) && headed.as_deref() != Some(bytes.as_slice()) {
        if *printed {
            out.push(b'\n');
        }
        if heading {
            out.extend_from_slice(b.display.as_bytes());
            out.push(b'\n');
        }
        *headed = Some(bytes.clone());
    }
    for (ln, row) in &rows {
        let Some(raw_l) = lines.get(ln.wrapping_sub(1)).copied() else {
            return Err("line moved");
        };
        let adjacent = last
            .as_ref()
            .is_some_and(|(loi, lp, ll)| *loi == oi && *lp == bytes && *ln <= ll + 1);

        if ctx > 0 && *printed && !adjacent {
            let same_file = last
                .as_ref()
                .is_some_and(|(loi, lp, _)| *loi == oi && *lp == bytes);
            if same_file || !(heading || file_blank) {
                out.extend_from_slice(b"--\n");
            }
        }
        if row.is_match {
            if plan.only_matching {
                let sh = String::from_utf8_lossy(raw_l);
                let sh = sh.trim_end();
                if sh.contains('\u{FFFD}') {
                    return Ok(None);
                }
                for r in &row.ranges {
                    let Some(seg) = sh.get(r.0..r.1) else {
                        return Ok(None);
                    };
                    rg_prefix(
                        out,
                        plan,
                        b,
                        show_name,
                        heading,
                        true,
                        numbered,
                        column.then_some(r.0 + 1),
                        *ln,
                    );
                    out.extend_from_slice(seg.as_bytes());
                    out.push(b'\n');
                }
            } else if plan.vimgrep {
                for r in &row.ranges {
                    rg_prefix(
                        out,
                        plan,
                        b,
                        show_name,
                        heading,
                        true,
                        numbered,
                        column.then_some(r.0 + 1),
                        *ln,
                    );
                    out.extend_from_slice(raw_l);
                    out.push(b'\n');
                }
            } else {
                let col = column.then(|| row.ranges.iter().map(|r| r.0).min().unwrap_or(0) + 1);
                rg_prefix(out, plan, b, show_name, heading, true, numbered, col, *ln);
                out.extend_from_slice(raw_l);
                out.push(b'\n');
            }
        } else {
            rg_prefix(out, plan, b, show_name, heading, false, numbered, None, *ln);
            out.extend_from_slice(raw_l);
            out.push(b'\n');
        }
        *printed = true;
        *last = Some((oi, bytes.clone(), *ln));
    }
    Ok(Some(()))
}

#[allow(clippy::too_many_arguments)]
fn rg_prefix(
    out: &mut Vec<u8>,
    plan: &RgPlan,
    b: &RgBlock,
    show_name: bool,
    heading: bool,
    is_match: bool,
    numbered: bool,
    col: Option<usize>,
    ln: usize,
) {
    if show_name && !heading {
        out.extend_from_slice(b.display.as_bytes());
        out.push(if plan.null_sep {
            0
        } else if is_match {
            b':'
        } else {
            b'-'
        });
    }
    if numbered {
        out.extend_from_slice(ln.to_string().as_bytes());
        out.push(if is_match { b':' } else { b'-' });
    }
    if let Some(c) = col {
        out.extend_from_slice(c.to_string().as_bytes());
        out.push(b':');
    }
}

#[path = "search_answer_tests.rs"]
#[cfg(test)]
mod tests;
