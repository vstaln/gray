#[cfg(target_os = "linux")]
use std::collections::{BTreeSet, HashMap};
#[cfg(target_os = "linux")]
use std::ffi::{OsStr, OsString};
#[cfg(target_os = "linux")]
use std::io::{BufRead, BufReader, Write};
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_os = "linux")]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};
#[cfg(target_os = "linux")]
use std::time::Instant;

#[cfg(target_os = "linux")]
#[cfg(target_os = "linux")]
use serde_json::{Value, json};

#[cfg(target_os = "linux")]
use crate::search_plan::{self, GrepPlan, RgPlan};
#[cfg(target_os = "linux")]
use crate::search_watch::Watch;

#[cfg(target_os = "linux")]
struct Server {
    root: PathBuf,
    engine: fsearch::Engine,
    watch: Arc<Watch>,

    order: Mutex<HashMap<PathBuf, Option<HashMap<OsString, usize>>>>,

    unheld: Mutex<Option<Arc<BTreeSet<Vec<u8>>>>>,

    _index_dir: Option<tempfile::TempDir>,
}

#[cfg(target_os = "linux")]
pub fn serve(root: &Path, sock: &Path, index_dir: Option<&Path>) -> std::io::Result<()> {
    let root = root.canonicalize()?;
    let home = root
        .to_str()
        .ok_or_else(|| std::io::Error::other("root is not UTF-8"))?
        .to_string();
    let (index_dir, _index_dir) = match index_dir {
        Some(d) => {
            std::fs::create_dir_all(d)?;
            if d.canonicalize()
                .unwrap_or_else(|_| d.to_path_buf())
                .starts_with(&root)
            {
                return Err(std::io::Error::other(
                    "index dir must not be inside the indexed root",
                ));
            }
            (d.to_path_buf(), None)
        }
        None => {
            let t = tempfile::Builder::new().prefix("gray-serve-").tempdir()?;
            (t.path().to_path_buf(), Some(t))
        }
    };

    let watch = Arc::new(Watch::new(&root)?);
    let drain = watch.clone();
    std::thread::spawn(move || drain.drain_forever());
    let engine = fsearch::Engine::start(fsearch::Options {
        dir: index_dir,
        home,
        root: Some(root.clone()),
        skip: Some(Vec::new()),
    })
    .map_err(std::io::Error::other)?;
    let server = Arc::new(Server {
        root,
        engine,
        watch,
        order: Mutex::new(HashMap::new()),
        unheld: Mutex::new(None),
        _index_dir,
    });
    let _ = std::fs::remove_file(sock);
    let listener = UnixListener::bind(sock)?;
    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let server = server.clone();
        std::thread::spawn(move || {
            let _ = answer(&server, stream);
        });
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn serve(_root: &Path, _sock: &Path, _index_dir: Option<&Path>) -> std::io::Result<()> {
    Err(std::io::Error::other("search-serve needs linux inotify"))
}

#[cfg(target_os = "linux")]
fn answer(server: &Server, mut stream: UnixStream) -> std::io::Result<()> {
    let mut line = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut line)?;
    let mut out = decide(server, &line).to_string();
    out.push('\n');
    stream.write_all(out.as_bytes())
}

#[cfg(target_os = "linux")]
fn decide(server: &Server, line: &str) -> Value {
    let started = Instant::now();
    let mut reply = match serve_line(server, line) {
        Ok(Answer::Exec { argv, files, mode }) => {
            json!({ "served": true, "argv": argv, "candidates": files.len(), "files": files, "mode": mode })
        }
        Ok(Answer::Print {
            stdout,
            stderr,
            code,
        }) => {
            use base64::Engine as _;
            json!({
                "served": true,
                "mode": "answer",
                "candidates": 0,
                "stdout": base64::engine::general_purpose::STANDARD.encode(&stdout),
                "stderr": base64::engine::general_purpose::STANDARD.encode(&stderr),
                "code": code,
            })
        }
        Err(why) => json!({ "served": false, "reason": why }),
    };
    reply["server_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
    reply
}

pub(crate) enum Answer {
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

#[cfg(target_os = "linux")]
fn serve_line(server: &Server, line: &str) -> Result<Answer, &'static str> {
    let req: Value = serde_json::from_str(line).map_err(|_| "bad request")?;
    let argv = req["argv"]
        .as_array()
        .and_then(|a| {
            a.iter()
                .map(|x| x.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or("bad request")?;
    let cwd = Path::new(req["cwd"].as_str().ok_or("bad request")?);
    match req["tool"].as_str() {
        Some("grep") => grep(server, &argv, cwd),
        Some("rg") => rg(server, &argv, cwd),
        _ => Err("tool not served"),
    }
}

enum Op {
    Indexed { scope: PathBuf, prefix: String },
    Pass(String),
}

type Unheld = Mutex<Option<Arc<BTreeSet<Vec<u8>>>>>;

struct Src<'a> {
    engine: &'a fsearch::Engine,

    root: &'a Path,
    watch: Option<&'a Watch>,
    unheld: Option<&'a Unheld>,
}

fn grep(server: &Server, argv: &[String], cwd: &Path) -> Result<Answer, &'static str> {
    let plan = search_plan::grep_plan(argv)?;
    let dbg = |m: &str| {
        if std::env::var_os("SERVE_DBG").is_some() {
            eprintln!("serve[{m}]");
        }
    };
    dbg("plan");
    let mut ops = Vec::new();
    let mut indexed = false;
    for raw in &plan.operands {
        let joined = cwd.join(raw);
        let scope = (plan.recursive && !raw.is_empty() && joined.is_dir())
            .then(|| joined.canonicalize().ok())
            .flatten();
        let serve = scope.as_ref().is_some_and(|s| {
            plan.all_needled
                && !plan.needles.is_empty()
                && s.starts_with(&server.root)
                && !s
                    .file_name()
                    .is_some_and(|n| plan.walk.skips_dir(n.as_bytes()))
        });
        if !serve {
            ops.push(Op::Pass(raw.clone()));
            continue;
        }
        let scope = scope.unwrap();
        let trimmed = raw.trim_end_matches('/');
        let prefix = if raw.starts_with('/') && trimmed.is_empty() {
            "/".to_string()
        } else {
            format!("{trimmed}/")
        };
        ops.push(Op::Indexed { scope, prefix });
        indexed = true;
    }
    let mut files = Vec::new();
    if indexed {
        dbg("ops");
        let st = server.engine.status();
        if !st.ready || st.root_missing || st.content_pending > 0 || st.content_docs == 0 {
            return Err("index not settled");
        }
        if plan.answerable() && ops.iter().all(|op| matches!(op, Op::Indexed { .. })) {
            let mut order = server.order.lock().unwrap();
            let mut unheld: Option<Arc<BTreeSet<Vec<u8>>>> = None;
            let src = Src {
                engine: &server.engine,
                root: &server.root,
                watch: Some(&server.watch),
                unheld: Some(&server.unheld),
            };
            if let Some(print) = print_answer(&src, &plan, &ops, &mut order, &mut unheld)? {
                return Ok(print);
            }
        }
        let mut order = server.order.lock().unwrap();
        let mut unheld: Option<Arc<BTreeSet<Vec<u8>>>> = None;
        for op in &ops {
            match op {
                Op::Pass(raw) => files.push(raw.clone()),
                Op::Indexed { scope, prefix } => files.extend(dir_files(
                    server,
                    scope,
                    prefix,
                    &plan,
                    &mut order,
                    &mut unheld,
                )?),
            }
        }
    } else {
        files = plan.operands.clone();
    }

    let mut out_argv = plan.argv.clone();

    if plan.recursive {
        out_argv.push("-r".into());
    }
    if plan.with_name.is_none() && (ops.len() > 1 || indexed) {
        out_argv.insert(0, "-H".into());
    }
    Ok(Answer::Exec {
        argv: out_argv,
        files,
        mode: if indexed { "index" } else { "pass" },
    })
}

fn dir_files(
    server: &Server,
    scope: &Path,
    prefix: &str,
    plan: &GrepPlan,
    order: &mut HashMap<PathBuf, Option<HashMap<OsString, usize>>>,
    unheld: &mut Option<Arc<BTreeSet<Vec<u8>>>>,
) -> Result<Vec<String>, &'static str> {
    let scope_bytes = scope.as_os_str().as_bytes();
    let changed = server.watch.changed(scope_bytes).ok_or("watch broken")?;
    if !changed.is_empty() {
        order.clear();
        *unheld = None;
        *server.unheld.lock().unwrap() = None;
    }
    if unheld.is_none() {
        let mut cache = server.unheld.lock().unwrap();
        if cache.is_none() {
            let root_bytes = server.root.as_os_str().as_bytes();
            let all: BTreeSet<Vec<u8>> = server
                .engine
                .unheld_files(root_bytes)
                .ok_or("content index not holding scope")?
                .into_iter()
                .collect();
            *cache = Some(Arc::new(all));
        }
        *unheld = Some(cache.as_ref().unwrap().clone());
    }
    let unheld = unheld
        .as_ref()
        .unwrap()
        .iter()
        .filter(|p| {
            p.starts_with(scope_bytes)
                && (p.len() == scope_bytes.len() || p.get(scope_bytes.len()) == Some(&b'/'))
        })
        .cloned();
    let matched = server
        .engine
        .held_matches(scope_bytes, &plan.needles, plan.ignore_case)
        .ok_or("match answer partial")?;
    let all: BTreeSet<Vec<u8>> = unheld.chain(matched).chain(changed).collect();
    let mut found = Vec::new();
    for p in all {
        let path = Path::new(OsStr::from_bytes(&p));
        let Ok(rel) = path.strip_prefix(scope) else {
            continue;
        };
        if !walked(plan, rel) || !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
            continue;
        }
        let name = rel.to_str().ok_or("path is not UTF-8")?;
        let key = walk_key(order, scope, rel).ok_or("walk order unknown")?;
        found.push((key, format!("{prefix}{name}")));
    }

    found.sort();
    Ok(found.into_iter().map(|(_, f)| f).collect())
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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
    src: &Src,
    plan: &GrepPlan,
    ops: &[Op],
    order: &mut HashMap<PathBuf, Option<HashMap<OsString, usize>>>,
    unheld: &mut Option<Arc<BTreeSet<Vec<u8>>>>,
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
            let (res, _idx) = src.engine.grep(&q, &g).map_err(|_| "index grep failed")?;
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
        let mut extra: BTreeSet<Vec<u8>> = match src.watch {
            Some(w) => w
                .changed(scope_bytes)
                .ok_or("watch broken")?
                .into_iter()
                .collect(),
            None => BTreeSet::new(),
        };
        match src.unheld {
            Some(cache) => {
                if unheld.is_none() {
                    let mut c = cache.lock().unwrap();
                    if c.is_none() {
                        if std::env::var_os("SERVE_DBG").is_some() {
                            eprintln!("serve[unheld_files]");
                        }
                        let all: BTreeSet<Vec<u8>> = src
                            .engine
                            .unheld_files(src.root.as_os_str().as_bytes())
                            .ok_or("content index not holding scope")?
                            .into_iter()
                            .collect();
                        *c = Some(Arc::new(all));
                    }
                    *unheld = Some(c.as_ref().unwrap().clone());
                }
                extra.extend(
                    unheld
                        .as_ref()
                        .unwrap()
                        .iter()
                        .filter(|p| {
                            p.starts_with(scope_bytes)
                                && (p.len() == scope_bytes.len()
                                    || p.get(scope_bytes.len()) == Some(&b'/'))
                        })
                        .cloned(),
                );
            }
            None => {
                if std::env::var_os("SERVE_DBG").is_some() {
                    eprintln!("serve[unheld_files]");
                }
                extra.extend(
                    src.engine
                        .unheld_files(scope_bytes)
                        .ok_or("content index not holding scope")?,
                );
            }
        }
        if std::env::var_os("SERVE_DBG").is_some() {
            eprintln!("serve[names]");
        }
        let known = src
            .engine
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
    let src = Src {
        engine,
        root: Path::new("/"),
        watch: None,
        unheld: None,
    };
    let mut order = HashMap::new();
    let mut unheld = None;
    match print_answer(&src, &plan, &ops, &mut order, &mut unheld) {
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
    let src = Src {
        engine,
        root: Path::new("/"),
        watch: None,
        unheld: None,
    };
    let mut unheld = None;
    match rg_print_answer(&src, &plan, &ops, &mut unheld) {
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

fn rg(server: &Server, argv: &[String], cwd: &Path) -> Result<Answer, &'static str> {
    let plan = search_plan::rg_plan(argv)?;

    let raws: Vec<String> = if plan.operands.is_empty() {
        let cwd_canon = cwd.canonicalize().map_err(|_| "cwd gone")?;
        if !plan.all_needled || plan.needles.is_empty() || !cwd_canon.starts_with(&server.root) {
            return Err("cwd search outside the index");
        }
        vec![".".to_string()]
    } else {
        plan.operands.clone()
    };
    let implicit = plan.operands.is_empty();
    let mut ops = Vec::new();
    let mut indexed = false;
    for raw in &raws {
        let joined = cwd.join(raw);
        let scope = (!raw.is_empty() && raw != "-" && joined.is_dir())
            .then(|| joined.canonicalize().ok())
            .flatten();
        let serve = scope.as_ref().is_some_and(|s| {
            plan.all_needled && !plan.needles.is_empty() && s.starts_with(&server.root)
        });
        let serve = if implicit { scope.is_some() } else { serve };
        if !serve {
            ops.push(Op::Pass(raw.clone()));
            continue;
        }
        let scope = scope.unwrap();

        let trimmed = raw.trim_end_matches('/');
        let prefix = if implicit {
            String::new()
        } else if raw.starts_with('/') && trimmed.is_empty() {
            "/".to_string()
        } else {
            format!("{trimmed}/")
        };
        ops.push(Op::Indexed { scope, prefix });
        indexed = true;
    }
    if plan.quiet && ops.iter().any(|op| matches!(op, Op::Pass(_))) {
        return Err("quiet over pass-through operands");
    }
    let mut files = Vec::new();
    if indexed {
        let st = server.engine.status();
        if !st.ready || st.root_missing || st.content_pending > 0 || st.content_docs == 0 {
            return Err("index not settled");
        }
        if plan.answerable() && ops.iter().all(|op| matches!(op, Op::Indexed { .. })) {
            let mut unheld: Option<Arc<BTreeSet<Vec<u8>>>> = None;
            let src = Src {
                engine: &server.engine,
                root: &server.root,
                watch: Some(&server.watch),
                unheld: Some(&server.unheld),
            };
            if let Some(print) = rg_print_answer(&src, &plan, &ops, &mut unheld)? {
                return Ok(print);
            }
        }
        let mut unheld: Option<Arc<BTreeSet<Vec<u8>>>> = None;
        for op in &ops {
            match op {
                Op::Pass(raw) => files.push(raw.clone()),
                Op::Indexed { scope, prefix } => {
                    for (display, abs) in rg_dir_files(server, scope, prefix, &plan, &mut unheld)? {
                        let buf = std::fs::read(&abs).map_err(|_| "file vanished")?;
                        if buf.contains(&0) {
                            let safe = plan.fsearch_grep().is_some_and(|g| !g.matches_buf(&buf));
                            if !safe {
                                return Err("binary candidate");
                            }
                        }
                        files.push(display);
                    }
                }
            }
        }
    } else {
        files = plan.operands.clone();
    }

    if ops.len() > 1 && files.len() < 2 {
        files.push("/dev/null".into());
    }
    let mut out_argv = plan.argv.clone();

    if plan.with_name.is_none() && (ops.len() > 1 || indexed) {
        out_argv.insert(0, "-H".into());
    }
    Ok(Answer::Exec {
        argv: out_argv,
        files,
        mode: if indexed { "index" } else { "pass" },
    })
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

fn rg_dir_files(
    server: &Server,
    scope: &Path,
    prefix: &str,
    plan: &RgPlan,
    unheld: &mut Option<Arc<BTreeSet<Vec<u8>>>>,
) -> Result<Vec<(String, PathBuf)>, &'static str> {
    let scope_bytes = scope.as_os_str().as_bytes();
    let changed = server.watch.changed(scope_bytes).ok_or("watch broken")?;
    if unheld.is_none() {
        let mut cache = server.unheld.lock().unwrap();
        if cache.is_none() {
            let root_bytes = server.root.as_os_str().as_bytes();
            let all: BTreeSet<Vec<u8>> = server
                .engine
                .unheld_files(root_bytes)
                .ok_or("content index not holding scope")?
                .into_iter()
                .collect();
            *cache = Some(Arc::new(all));
        }
        *unheld = Some(cache.as_ref().unwrap().clone());
    }
    let insensitive = plan.insensitive();
    let mut cands: BTreeSet<Vec<u8>> = server
        .engine
        .held_matches(scope_bytes, &plan.needles, insensitive)
        .ok_or("match answer partial")?
        .into_iter()
        .collect();
    cands.extend(changed);
    cands.extend(
        unheld
            .as_ref()
            .unwrap()
            .iter()
            .filter(|p| {
                p.starts_with(scope_bytes)
                    && (p.len() == scope_bytes.len() || p.get(scope_bytes.len()) == Some(&b'/'))
            })
            .cloned(),
    );
    let known: std::collections::HashSet<Vec<u8>> = server
        .engine
        .search(&fsearch::Query {
            scope: Some(scope_bytes.to_vec()),
            name_re: Some(regex::bytes::Regex::new(".*").map_err(|_| "re")?),
            size: (0, u64::MAX),
            mtime: (0, u32::MAX),
            limit: 1 << 24,
            ..Default::default()
        })
        .map_err(|_| "index list failed")?
        .into_iter()
        .map(|f| f.path.as_os_str().as_bytes().to_vec())
        .collect();
    let mut out = Vec::new();
    for item in rg_walk(scope, plan)? {
        match item {
            WalkItem::File(path) => {
                let bytes = path.as_os_str().as_bytes();
                if !cands.contains(bytes) && known.contains(bytes) {
                    continue;
                }
                let Ok(rel) = path.strip_prefix(scope) else {
                    continue;
                };
                let Some(name) = rel.to_str() else {
                    return Err("path is not UTF-8");
                };
                out.push((rg_display(prefix, name, plan), path));
            }
            WalkItem::Err(..) => return Err("walk error"),
        }
    }
    Ok(out)
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
    src: &Src,
    plan: &RgPlan,
    ops: &[Op],
    unheld: &mut Option<Arc<BTreeSet<Vec<u8>>>>,
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
            let (res, _idx) = src.engine.grep(&q, &g).map_err(|_| "index grep failed")?;
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

        let mut extra: BTreeSet<Vec<u8>> = match src.watch {
            Some(w) => w
                .changed(scope_bytes)
                .ok_or("watch broken")?
                .into_iter()
                .collect(),
            None => BTreeSet::new(),
        };
        match src.unheld {
            Some(cache) => {
                if unheld.is_none() {
                    let mut c = cache.lock().unwrap();
                    if c.is_none() {
                        let all: BTreeSet<Vec<u8>> = src
                            .engine
                            .unheld_files(src.root.as_os_str().as_bytes())
                            .ok_or("content index not holding scope")?
                            .into_iter()
                            .collect();
                        *c = Some(Arc::new(all));
                    }
                    *unheld = Some(c.as_ref().unwrap().clone());
                }
                extra.extend(
                    unheld
                        .as_ref()
                        .unwrap()
                        .iter()
                        .filter(|p| {
                            p.starts_with(scope_bytes)
                                && (p.len() == scope_bytes.len()
                                    || p.get(scope_bytes.len()) == Some(&b'/'))
                        })
                        .cloned(),
                );
            }
            None => {
                extra.extend(
                    src.engine
                        .unheld_files(scope_bytes)
                        .ok_or("content index not holding scope")?,
                );
            }
        }
        let known: std::collections::HashSet<Vec<u8>> = src
            .engine
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

#[path = "search_serve_tests.rs"]
#[cfg(test)]
mod tests;
