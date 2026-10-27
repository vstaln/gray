use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const WAIT_INDEX: Duration = Duration::from_secs(30);

const MAX_ASKS: usize = 16;

const INSTRUCTIONS: &str = "fsearch: indexed search over this project (or any folder, or the whole disk with path \"/\").
- grep: file contents; the default for any identifier or text. Definitions come first, with their first lines.
- find_files: file names (fuzzy, typo-tolerant), to locate a file or see what exists.
- multi_grep: several spellings at once (snake_case, CamelCase), OR'd.
Filters go inline: *.rs or *.{ts,tsx}, src/ (in a folder), src*.rs (glob), !test/ or !*.md (exclude), \
git:modified|staged|untracked (git status); grep also takes a file name (main.rs foo). Results are best first: \
read the top file instead of searching again.";

pub fn run(dir: PathBuf) {
    let mut s = Session::new(dir);
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) if batch.is_empty() => Some(error(&Value::Null, -32600, "invalid request: an empty batch")),
            Ok(Value::Array(batch)) => {
                let r: Vec<Value> = batch.iter().filter_map(|m| s.message(m)).collect();
                (!r.is_empty()).then_some(Value::Array(r))
            }
            Ok(m) => s.message(&m),
            Err(e) => Some(error(&Value::Null, -32700, &format!("parse error: {e}"))),
        };
        for m in reply.into_iter().chain(s.outbox.drain(..)) {
            if writeln!(out, "{m}").and_then(|_| out.flush()).is_err() {
                return;
            }
        }
    }
}

struct Session {
    dir: PathBuf,
    home: String,

    root: String,

    outbox: Vec<Value>,
    asked_roots: u32,

    asks: std::cell::Cell<usize>,
}

fn error(id: &Value, code: i64, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

fn result(id: &Value, r: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": r})
}

impl Session {
    fn new(dir: PathBuf) -> Session {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| home.clone());

        let real = |p: String| std::fs::canonicalize(&p).map_or(p, |c| c.to_string_lossy().into_owned());
        let root = real(repo_root(&cwd, &home).unwrap_or(cwd));
        Session { dir, home, root, outbox: Vec::new(), asked_roots: 0, asks: Default::default() }
    }

    fn message(&mut self, m: &Value) -> Option<Value> {
        let id = m.get("id");
        let Some(method) = m.get("method").and_then(Value::as_str) else {
            if m.get("result").is_none() && m.get("error").is_none() {
                return Some(error(id.unwrap_or(&Value::Null), -32600, "invalid request: no method"));
            }

            if let Some(roots) = m.pointer("/result/roots").and_then(Value::as_array)
                && let Some(p) = roots.iter().find_map(|r| r["uri"].as_str().and_then(file_uri))
            {
                self.root = std::fs::canonicalize(&p).map_or(p, |c| c.to_string_lossy().into_owned());
            }
            return None;
        };
        let params = m.get("params").unwrap_or(&Value::Null);
        let Some(id) = id else {
            match method {
                "notifications/initialized" | "notifications/roots/list_changed" if self.asked_roots > 0 => self.ask_roots(),
                _ => {}
            }
            return None;
        };
        Some(match method {
            "initialize" => {
                let want = params["protocolVersion"].as_str().unwrap_or("");
                let version = VERSIONS.iter().find(|v| **v == want).unwrap_or(&VERSIONS[0]);
                if params.pointer("/capabilities/roots").is_some() {
                    self.asked_roots = 1;
                }
                self.warm();
                result(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "fsearch", "version": env!("CARGO_PKG_VERSION")},
                        "instructions": INSTRUCTIONS,
                    }),
                )
            }
            "ping" => result(id, json!({})),
            "tools/list" => result(id, json!({ "tools": tools() })),
            "tools/call" => {
                let args = params.get("arguments").unwrap_or(&Value::Null);
                self.asks.set(0);
                let r = match params["name"].as_str().unwrap_or("") {
                    "find_files" => self.find_files(args),
                    "grep" => self.grep(args, false),
                    "multi_grep" => self.grep(args, true),
                    other => return Some(error(id, -32602, &format!("unknown tool {other}"))),
                };
                let (text, is_error) = match r {
                    Ok(t) => (t, false),
                    Err(e) => (e, true),
                };
                result(id, json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
            }
            _ => error(id, -32601, &format!("method not found: {method}")),
        })
    }

    fn warm(&self) {
        let dir = self.dir.clone();
        std::thread::spawn(move || ask_at(&dir, &json!({"op": "ping"})).ok());
    }

    fn ask_roots(&mut self) {
        self.asked_roots += 1;
        self.outbox.push(json!({"jsonrpc": "2.0", "id": format!("fsearch-roots-{}", self.asked_roots), "method": "roots/list"}));
    }

    fn ask(&self, req: &Value) -> Result<Value, String> {
        self.spend(1)?;
        ask_at(&self.dir, req)
    }

    fn ask2(&self, a: &Value, b: &Value) -> (Result<Value, String>, Result<Value, String>) {
        if let Err(e) = self.spend(2) {
            return (Err(e.clone()), Err(e));
        }
        (ask_at(&self.dir, a), ask_at(&self.dir, b))
    }

    fn spend(&self, n: usize) -> Result<(), String> {
        let used = self.asks.get() + n;
        if used > MAX_ASKS {
            return Err(format!("stopped after {MAX_ASKS} searches for one call"));
        }
        self.asks.set(used);
        Ok(())
    }

    fn scope(&self, a: &Value) -> Option<String> {
        let p = a["path"].as_str().map(str::trim).filter(|p| !p.is_empty() && *p != ".").unwrap_or(&self.root);
        Some(match p {
            "/" => return None,
            p if p.starts_with('/') => p.trim_end_matches('/').to_string(),
            p if p == "~" || p.starts_with("~/") => format!("{}{}", self.home, &p[1..]).trim_end_matches('/').to_string(),
            p => format!("{}/{}", self.root, p.trim_start_matches("./").trim_end_matches('/')),
        })
    }

    fn view(&self, scope: &Option<String>) -> View {
        match scope {
            Some(s) if !within(s, &self.root) => View { base: s.clone(), head: Some(format!("in {s}/:")) },
            _ => View { base: self.root.clone(), head: None },
        }
    }

    fn find_files(&self, a: &Value) -> Result<String, String> {
        let query = arg_str(a, &["query", "pattern"]).unwrap_or_default();
        let max = arg_num(a, "maxResults", 20);
        let offset = cursor(a)?;
        let scope = self.scope(a);
        if query.trim().is_empty() || query.trim() == "*" {
            return self.worked_on(&scope, max);
        }
        let mut p = split(&query, false);

        let dirs = p.words.is_empty() && p.globs.len() == 1 && p.globs[0].ends_with('/') && !p.globs[0].starts_with('!');
        if dirs {
            let d = p.globs.pop().unwrap();
            p.words.push(d.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string());
        }
        let req = |words: &[String], hide: bool| {
            let mut r = json!({
                "q": words.iter().chain(&p.filters).cloned().collect::<Vec<_>>().join(" "),
                "limit": max, "offset": offset, "git": true, "boost": true,
            });

            if hide && !query.contains(".git") {
                r["ignored"] = false.into();
            }
            if !p.filters.iter().any(|f| f.starts_with("kind:")) {
                r["kind"] = (if dirs { "dir" } else { "file" }).into();
            }
            if let Some(s) = &scope {
                r["in"] = s.as_str().into();
            }

            let mut globs = p.globs.clone();
            if !query.contains(".git") && (!hide || !Path::new(&self.root).join(".git").exists()) {
                globs.push("!.git/".into());
            }
            if !globs.is_empty() {
                r["glob"] = json!(globs);
            }
            r
        };
        let v = self.view(&scope);
        let r = self.ask(&req(&p.words, true))?;
        let hits = r["hits"].as_array().cloned().unwrap_or_default();
        if hits.is_empty() && offset > 0 {
            return Ok(format!("no more files past cursor {offset}"));
        }
        if !hits.is_empty() {
            return Ok(v.list_files(&hits, &p.words, r["next"].as_u64(), None));
        }

        for (i, w) in p.words.iter().enumerate().filter(|(_, w)| (3..5).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_alphanumeric())) {
            for j in 0..w.len() - 1 {
                let mut t = w.clone().into_bytes();
                t.swap(j, j + 1);
                let mut words = p.words.clone();
                words[i] = String::from_utf8(t).unwrap_or_default();
                if words[i] == *w {
                    continue;
                }
                let Ok(r) = self.ask(&req(&words, true)) else { break };
                let hits = r["hits"].as_array().cloned().unwrap_or_default();
                if !hits.is_empty() {
                    return Ok(v.list_files(&hits, &words, r["next"].as_u64(), Some(format!("0 files match '{w}'; as '{}':", words[i]))));
                }
            }
        }

        let r = self.ask(&req(&p.words, false)).unwrap_or_default();
        let hits = r["hits"].as_array().cloned().unwrap_or_default();
        if !hits.is_empty() {
            return Ok(v.list_files(&hits, &p.words, r["next"].as_u64(), Some("0 matches outside git-ignored files; ignored:".into())));
        }

        if p.words.len() > 1 {
            for i in (0..p.words.len()).rev() {
                let fewer: Vec<String> = p.words.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, w)| w.clone()).collect();
                let Ok(r) = self.ask(&req(&fewer, true)) else { break };
                let hits = r["hits"].as_array().cloned().unwrap_or_default();
                if !hits.is_empty() {
                    let note = format!("0 files match all of '{}'; without '{}':", p.words.join(" "), p.words[i]);
                    return Ok(v.list_files(&hits, &fewer, r["next"].as_u64(), Some(note)));
                }
            }
        }

        let text = p.words.join(" ");
        if !text.is_empty() {
            let mut g = json!({"op": "grep", "pattern": text, "limit": 5, "per_file": 1, "budget_ms": 500});
            if let Some(s) = &scope {
                g["in"] = s.as_str().into();
            }
            let mut d = g.clone();
            (d["mode"], d["limit"]) = ("symbol".into(), 2.into());
            let (r, d) = self.ask2(&g, &d);
            let name = def_name(&text);
            let defs = d.map(|d| files(&d).to_vec()).unwrap_or_default();
            let mut fs: Vec<Value> = defs.into_iter().filter(|f| is_def(f["matches"][0]["text"].as_str().unwrap_or(""), name)).collect();
            for f in r.map(|r| files(&r).to_vec()).unwrap_or_default() {
                if fs.len() < 5 && !fs.iter().any(|o| o["path"] == f["path"]) {
                    fs.push(f);
                }
            }
            if !fs.is_empty() {
                let mut out: Vec<String> = v.head.iter().cloned().collect();
                out.push(format!("0 file names match '{text}'; files containing it:"));
                for f in fs {
                    let (path, m) = (f["path"].as_str().unwrap_or(""), &f["matches"][0]);
                    out.push(format!("{}:{}: {}", v.show(path), m["line"], clip(m["text"].as_str().unwrap_or(""), &text, 120)));
                }
                return Ok(out.join("\n"));
            }
        }
        Ok(format!("0 files match '{query}' in {}", scope.as_deref().unwrap_or("the whole disk")))
    }

    fn worked_on(&self, scope: &Option<String>, max: usize) -> Result<String, String> {
        let mut changed = json!({"q": "git:modified", "kind": "file", "limit": max, "git": true, "boost": true});
        let mut recent = json!({"op": "recent", "kind": "file", "limit": max, "git": true});
        if let Some(s) = scope {
            (changed["in"], recent["in"]) = (s.as_str().into(), s.as_str().into());
        }
        let mut hits = self.ask(&changed).ok().and_then(|r| r["hits"].as_array().cloned()).unwrap_or_default();
        hits.truncate(max);
        for h in self.ask(&recent)?["hits"].as_array().cloned().unwrap_or_default() {
            if hits.len() < max && !hits.iter().any(|o| o["path"] == h["path"]) {
                hits.push(h);
            }
        }
        if hits.is_empty() {
            return Ok("nothing changed in git or worked on lately here; give a query".into());
        }
        Ok(self.view(scope).list_files(&hits, &[], None, Some("changed, then worked on lately:".into())))
    }

    fn grep(&self, a: &Value, multi: bool) -> Result<String, String> {
        let max = arg_num(a, "maxResults", 20);
        let offset = cursor(a)?;
        let context = arg_num(a, "context", 0).min(20);
        let mode = Out::of(a["output_mode"].as_str());
        let scope = self.scope(a);
        let (p, pattern, regex) = if multi {
            let pats = patterns(&a["patterns"]);
            if pats.is_empty() {
                return Err("multi_grep needs patterns".into());
            }
            let p = split(a["constraints"].as_str().unwrap_or(""), true);
            let alt = pats.iter().map(|s| regex::escape(s)).collect::<Vec<_>>().join("|");
            (p, if pats.len() == 1 { pats[0].clone() } else { alt }, pats.len() > 1)
        } else {
            let query = arg_str(a, &["query", "pattern"]).ok_or("grep needs a query")?;
            let mut p = split(&query, true);

            if p.words.is_empty() {
                p = Parts { words: vec![query.trim().to_string()], ..Default::default() };
            }
            let text = p.words.join(" ");
            let regex = a["regex"].as_bool().unwrap_or_else(|| looks_like_regex(&text));
            (p, text, regex)
        };
        let req = |pattern: &str, regex: bool, globs: &[String], offset: usize| {
            let mut r = json!({
                "op": "grep", "pattern": pattern, "mode": if regex { "regex" } else { "literal" },
                "q": p.filters.join(" "), "offset": offset, "budget_ms": 2000, "git": true, "ignored": false,

                "limit": if matches!(mode, Out::Content) { max / 2 + 1 } else { max },
                "per_file": match mode { Out::Count => 1000, Out::Files => 1, Out::Content => if globs.iter().any(|g| is_file_glob(g)) { max } else { 5 } },
            });
            if let Some(s) = &scope {
                r["in"] = s.as_str().into();
            }
            if !globs.is_empty() {
                r["glob"] = json!(globs);
            }
            r
        };
        let v = self.view(&scope);

        let name = def_name(&pattern).filter(|_| offset == 0 && matches!(mode, Out::Content) && !regex && !multi);
        let def_req = |pat: &str, regex: bool, mode: &str, limit: usize, budget: u64| {
            let mut d = req(pat, regex, &p.globs, 0);
            (d["mode"], d["limit"], d["per_file"], d["budget_ms"]) = (mode.into(), limit.into(), 2.into(), budget.into());
            d
        };
        let both = |a: Value, b: Value| self.ask2(&a, &b);
        let (main, sym) = match name {
            Some(n) => both(req(&pattern, regex, &p.globs, offset), def_req(n, false, "symbol", 3, 2000)),
            None => (self.ask(&req(&pattern, regex, &p.globs, offset)), Err(String::new())),
        };
        let mut r = match main {
            Err(e) if regex && !multi && a["regex"].as_bool().is_none() && e.contains("regex") => {
                self.ask(&req(&pattern, false, &p.globs, offset))?
            }
            r => r?,
        };
        let mut note = None;
        if offset > 0 && files(&r).is_empty() {
            return Ok(format!("no more matches past cursor {offset}"));
        }
        if offset == 0 && files(&r).is_empty() {
            let ident = !regex && is_ident(&pattern);
            let tries: Vec<(String, bool, Vec<String>, String)> = [
                p.globs.iter().any(|g| is_file_glob(g)).then(|| {
                    let globs = p.globs.iter().filter(|g| !is_file_glob(g)).cloned().collect();
                    (pattern.clone(), regex, globs, format!("0 matches for '{pattern}' in the named file; everywhere:"))
                }),
                (!regex && looks_like_regex(&pattern))
                    .then(|| (pattern.clone(), true, p.globs.clone(), format!("0 matches for '{pattern}' as text; as a regex:"))),
                ident.then(|| (variants(&pattern), true, p.globs.clone(), format!("0 matches for '{pattern}'; other spellings:"))),
                (!regex && pattern.contains(' ')).then(|| {
                    let w = pattern.split_whitespace().max_by_key(|w| w.len()).unwrap_or("").to_string();
                    (w.clone(), false, p.globs.clone(), format!("0 matches for '{pattern}'; for '{w}':"))
                }),
            ]
            .into_iter()
            .flatten()
            .collect();
            for (pat, re, globs, why) in tries {
                if let Ok(r2) = self.ask(&req(&pat, re, &globs, 0))
                    && !files(&r2).is_empty()
                {
                    if ident
                        && !multi
                        && pat == variants(&pattern)
                        && let Some(w) = spelling(&r2, &pat).filter(|w| *w != pattern)
                    {
                        let mut a2 = a.clone();
                        a2["query"] = arg_str(a, &["query", "pattern"]).unwrap_or_default().replacen(&pattern, &w, 1).into();
                        return Ok(format!("0 matches for '{pattern}'; as '{w}':\n{}", self.grep(&a2, false)?));
                    }
                    (r, note) = (r2, Some(why));
                    break;
                }
            }
            if note.is_none() {
                let mut f = json!({"q": pattern, "kind": "file", "limit": 3});
                if let Some(s) = &scope {
                    f["in"] = s.as_str().into();
                }
                let names = self.ask(&f).ok().and_then(|r| r["hits"].as_array().cloned()).unwrap_or_default();
                let mut out = format!("0 matches for '{pattern}' in {}", scope.as_deref().unwrap_or("the whole disk"));
                if r["indexing"].as_u64().is_some_and(|n| n > 0) {
                    out += " (the content index is still being built)";
                }
                if !names.is_empty() {
                    out += "; files named like it:";
                    for h in names {
                        out += &format!("\n{}", v.show(h["path"].as_str().unwrap_or("")));
                    }
                }
                return Ok(out);
            }
        }
        if let Some(name) = name {
            let only_defs = |r: &Result<Value, String>| {
                let mut fs = r.as_ref().map(|d| files(d).to_vec()).unwrap_or_default();
                for f in &mut fs {
                    let keep: Vec<Value> = f["matches"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|m| is_def(m["text"].as_str().unwrap_or(""), Some(name)))
                        .cloned()
                        .collect();
                    f["matches"] = keep.into();
                }
                fs.retain(|f| f["matches"].as_array().is_some_and(|m| !m.is_empty()));
                fs
            };
            let mut defs = only_defs(&sym);

            let c = |f: &Value| {
                f["path"].as_str().and_then(|p| p.rsplit_once('.')).is_some_and(|(_, e)| ["c", "h", "cc", "cpp", "hpp", "m", "mm"].contains(&e))
            };
            let good = |f: &Value| c(f) && !aux(v.show(f["path"].as_str().unwrap_or("")));
            if files(&r).iter().any(c) && !defs.iter().any(good) {
                let fun = format!(r"(?m-i)^(?:[A-Za-z_][A-Za-z0-9_ \t*]*[ \t*])?{}\(", regex::escape(name));
                let code = |mut d: Value| {
                    d["glob"] = json!(p.globs.iter().cloned().chain(AUX.iter().map(|a| format!("!{a}/"))).collect::<Vec<_>>());
                    d
                };
                let mut near = code(def_req(&fun, true, "regex", 4, 10));
                let words: Vec<String> = name.split('_').filter(|w| w.len() >= 3).map(regex::escape).collect();
                near["path"] = format!(
                    "^{}/.*(?:{})",
                    regex::escape(scope.as_deref().unwrap_or("")),
                    if words.is_empty() { regex::escape(name) } else { words.join("|") }
                )
                .into();
                let (st, nr) = both(def_req(&format!("struct {name} {{"), false, "literal", 8, 2000), near);

                let mut cs: Vec<Value> = only_defs(&st).into_iter().chain(only_defs(&nr)).collect();
                if !cs.iter().any(good) {
                    cs.extend(only_defs(&self.ask(&code(def_req(&fun, true, "regex", 8, 25)))));
                }
                defs.splice(0..0, cs);
            }

            defs.extend(only_defs(&Ok(r.clone())));
            defs.retain(|f| !prototype(f["path"].as_str().unwrap_or(""), &f["matches"][0]));
            let rank = |f: &Value| def_rank(v.show(f["path"].as_str().unwrap_or("")), f["matches"][0]["text"].as_str().unwrap_or(""), name);
            defs.sort_by_key(rank);
            let mut seen = std::collections::HashSet::new();
            defs.retain(|f| seen.insert(f["path"].as_str().unwrap_or("").to_string()));

            let best = defs.first().map(|f| rank(f).0);
            defs.retain(|f| Some(rank(f).0) == best);
            defs.truncate(2);
            lead(&mut r, &defs);
        }
        Ok(v.list_matches(&r, &pattern, regex, if regex { None } else { def_name(&pattern) }, mode, max, context, offset, note))
    }
}

fn lead(r: &mut Value, defs: &[Value]) {
    let Some(fs) = r["files"].as_array_mut() else { return };

    for (i, f) in fs.iter_mut().enumerate() {
        f["at"] = i.into();
    }
    for d in defs.iter().rev() {
        let mut f = match fs.iter().position(|f| f["path"] == d["path"]) {
            Some(i) => fs.remove(i),
            None => json!({"path": d["path"], "matches": [], "extra": true}),
        };
        let mut ls: Vec<Value> = f["matches"].as_array().cloned().unwrap_or_default();
        ls.extend(d["matches"].as_array().cloned().unwrap_or_default());
        ls.sort_by_key(|m| m["line"].as_u64());
        ls.dedup_by_key(|m| m["line"].as_u64());
        f["matches"] = ls.into();
        if f.get("git").is_none() && d.get("git").is_some() {
            f["git"] = d["git"].clone();
        }
        f["lead"] = true.into();
        fs.insert(0, f);
    }
}

struct View {
    base: String,
    head: Option<String>,
}

impl View {
    fn show<'a>(&self, path: &'a str) -> &'a str {
        match path.strip_prefix(self.base.as_str()) {
            Some(rest) if rest.starts_with('/') => &rest[1..],
            _ => path,
        }
    }

    fn list_files(&self, hits: &[Value], words: &[String], next: Option<u64>, note: Option<String>) -> String {
        let at = |h: &&Value| {
            let p = self.show(h["path"].as_str().unwrap_or(""));
            words.iter().any(|w| w.contains('/') && (p == w.trim_start_matches("./") || p.ends_with(&format!("/{}", w.trim_start_matches("./")))))
        };
        let mut hits: Vec<&Value> = hits.iter().collect();
        hits.sort_by_key(|h| !at(h));

        let spelled = |h: &Value| {
            let p = h["path"].as_str().unwrap_or("").to_lowercase();
            words.iter().all(|w| p.contains(&w.to_lowercase()))
        };
        let weak = note.is_none() && !words.is_empty() && hits.first().is_some_and(|h| !spelled(h));
        let mut out: Vec<String> = self.head.iter().cloned().chain(note).collect();
        if weak {
            out.push(format!("no exact match for '{}'; closest:", words.join(" ")));
        }
        for h in hits {
            let path = self.show(h["path"].as_str().unwrap_or(""));
            let dir = if h["kind"] == "dir" { "/" } else { "" };
            let git = h["git"].as_str().map_or(String::new(), |g| format!(" ({g})"));
            out.push(format!("{path}{dir}{git}"));
        }
        if let Some(n) = next {
            out.push(format!("cursor: {n}"));
        }
        out.join("\n")
    }

    #[allow(clippy::too_many_arguments)]
    fn list_matches(
        &self,
        r: &Value,
        pattern: &str,
        regex: bool,
        name: Option<&str>,
        mode: Out,
        max: usize,
        context: usize,
        offset: usize,
        note: Option<String>,
    ) -> String {
        let mut out: Vec<String> = self.head.iter().cloned().chain(note).collect();
        let fs: Vec<&Value> = files(r).iter().collect();
        let head = |f: &Value| {
            let git = f["git"].as_str().map_or(String::new(), |g| format!(" ({g})"));
            format!("{}{git}", self.show(f["path"].as_str().unwrap_or("")))
        };
        let lines = |f: &Value| f["matches"].as_array().cloned().unwrap_or_default();

        let mut ranked: Vec<(usize, &Value)> =
            fs.iter().enumerate().filter(|(_, f)| f.get("extra").is_none()).map(|(i, f)| (f["at"].as_u64().map_or(i, |a| a as usize), *f)).collect();
        ranked.sort_by_key(|&(at, _)| at);
        let mut taken = ranked.len();
        match mode {
            Out::Files => out.extend(fs.iter().map(|f| head(f))),
            Out::Count => out.extend(fs.iter().map(|f| {
                let n = lines(f).len();
                format!("{}: {n}{}", head(f), if n >= 1000 { "+" } else { "" })
            })),
            Out::Content => {
                let text = |m: &Value| m["text"].as_str().unwrap_or("").to_string();
                let def = |f: &Value, m: &Value| is_def(&text(m), name) && !prototype(f["path"].as_str().unwrap_or(""), m);

                let rank = |f: &&Value| {
                    let ls = lines(f);
                    if f.get("lead").is_some() {
                        return 0;
                    }
                    if ls.iter().all(|m| is_import(&text(m))) {
                        return 4;
                    }
                    2 * aux(self.show(f["path"].as_str().unwrap_or(""))) as u8 + !ls.iter().any(|m| def(f, m)) as u8
                };

                let lead = |f: &Value| f.get("lead").is_some();
                let mut n: usize = fs.iter().filter(|f| lead(f)).map(|f| lines(f).len()).sum();
                taken = 0;
                while taken < ranked.len() && (taken == 0 || n < max) {
                    n += if lead(ranked[taken].1) { 0 } else { lines(ranked[taken].1).len() };
                    taken += 1;
                }
                let mut page: Vec<&Value> = ranked[..taken].iter().map(|&(_, f)| f).filter(|f| !lead(f)).collect();
                page.sort_by_cached_key(rank);
                let mut expanded = 0;
                for f in fs.iter().copied().filter(|f| lead(f)).chain(page) {
                    out.push(head(f));
                    let ls = lines(f);
                    let path = f["path"].as_str().unwrap_or("");

                    let body_of = |m: &Value, done: usize| done < 2 && (done == 0 || !aux(self.show(path))) && def(f, m) && opens_body(&text(m));

                    let text = if context > 0 || ls.iter().any(|m| body_of(m, expanded)) { read_text(path) } else { String::new() };
                    let body: Vec<&str> = text.lines().collect();
                    let mut last = 0;
                    for m in &ls {
                        let (ln, text) = (m["line"].as_u64().unwrap_or(0) as usize, m["text"].as_str().unwrap_or(""));
                        if ln <= last {
                            continue;
                        }
                        if context > 0 && !body.is_empty() {
                            for c in ln.saturating_sub(context).max(last + 1)..ln.min(body.len() + 1) {
                                out.push(format!("{c}-{}", clip(body[c - 1], "", 160)));
                            }
                        }
                        out.push(format!("{ln}:{}", clip(text, if regex { "" } else { pattern }, 160)));
                        last = ln;
                        let after = if context > 0 {
                            context
                        } else if body_of(m, expanded) {
                            expanded += 1;
                            6
                        } else {
                            0
                        };
                        for c in ln + 1..=(ln + after).min(body.len()) {
                            if context == 0 && body[c - 1].trim().is_empty() {
                                break;
                            }
                            if ls.iter().any(|m| m["line"].as_u64() == Some(c as u64)) {
                                break;
                            }
                            out.push(format!("{c}-{}", clip(body[c - 1], "", 160)));
                            last = c;
                        }
                    }
                }
            }
        }
        if r["next"].as_u64().is_some() || taken < ranked.len() {
            out.push(format!("cursor: {}", offset + taken));
        }
        if r["complete"] == false {
            out.push("(stopped early: more files may match; narrow it down)".into());
        }
        if r["indexing"].as_u64().is_some_and(|n| n > 0) {
            out.push("(the content index is still being built: results may be incomplete)".into());
        }
        out.join("\n")
    }
}

fn ask_at(dir: &Path, req: &Value) -> Result<Value, String> {
    let t = Instant::now();
    loop {
        let mut s = crate::server::connect(dir).map_err(|e| format!("cannot reach the fsearch daemon: {e}"))?;
        s.write_all(format!("{req}\n").as_bytes()).map_err(|e| e.to_string())?;
        let _ = s.shutdown(std::net::Shutdown::Write);
        let mut line = String::new();
        BufReader::new(&s).read_line(&mut line).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("daemon: {e}"))?;
        if v["ok"] == true {
            return Ok(v);
        }
        let e = v["error"].as_str().unwrap_or("error");
        if !e.starts_with("indexing") || t.elapsed() > WAIT_INDEX {
            return Err(e.to_string());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn files(r: &Value) -> &[Value] {
    r["files"].as_array().map_or(&[], Vec::as_slice)
}

#[derive(Clone, Copy)]
enum Out {
    Content,
    Files,
    Count,
}

impl Out {
    fn of(s: Option<&str>) -> Out {
        match s {
            Some("files_with_matches" | "files") => Out::Files,
            Some("count") => Out::Count,
            _ => Out::Content,
        }
    }
}

fn tools() -> Value {
    let common = |extra: Value| {
        let mut p = json!({
            "path": {"type": "string", "description": "Folder to search: relative to the project, absolute, or \"/\" for the whole disk. Default: the project."},
            "maxResults": {"type": "number", "description": "Default 20."},
            "cursor": {"type": "string", "description": "From the previous result, for the next page."},
        });
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        p
    };
    let grep_extra = json!({
        "context": {"type": "number", "description": "Lines to show around each match."},
        "output_mode": {"type": "string", "enum": ["content", "files_with_matches", "count"], "description": "Default content."},
    });
    let ro = json!({"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false});
    json!([
        {
            "name": "find_files",
            "description": "Find files by name: fuzzy and typo-tolerant, over the whole path. Use it to locate a file or see which files exist; use grep for contents. All words must match. Inline filters: *.rs, src/, src/**/*.rs, !test/, git:modified. A lone 'name/' finds folders; an empty query lists files changed in git, then those worked on lately.",
            "inputSchema": {"type": "object", "properties": common(json!({
                "query": {"type": "string", "description": "Name words and filters, e.g. 'config *.ts !test/'."},
            })), "required": ["query"]},
            "annotations": ro,
        },
        {
            "name": "grep",
            "description": "Search file contents (indexed). Give a bare identifier or literal text from one line, not code syntax; a regex is detected (\\b, .*, a|b, [a-z]) or set with regex. Smart case. Prefix filters: *.rs, src/, !test/, or a file name to search one file ('main.rs parse'). Definitions come first, with their first lines.",
            "inputSchema": {"type": "object", "properties": common({
                let mut g = json!({
                    "query": {"type": "string", "description": "Filters, then the text, e.g. '*.rs fn parse'."},
                    "regex": {"type": "boolean", "description": "Treat the text as a regex (default: detected)."},
                });
                g.as_object_mut().unwrap().extend(grep_extra.as_object().unwrap().clone());
                g
            }), "required": ["query"]},
            "annotations": ro,
        },
        {
            "name": "multi_grep",
            "description": "Search file contents for lines matching ANY of several literal patterns (OR), e.g. every spelling of a name: ['PrepareUpload', 'prepare_upload']. Patterns are literal: never escape them. Filters go in constraints.",
            "inputSchema": {"type": "object", "properties": common({
                let mut g = json!({
                    "patterns": {"type": "array", "items": {"type": "string"}, "description": "Literal patterns, any of which may match."},
                    "constraints": {"type": "string", "description": "File filters, e.g. '*.{ts,tsx} !test/'."},
                });
                g.as_object_mut().unwrap().extend(grep_extra.as_object().unwrap().clone());
                g
            }), "required": ["patterns"]},
            "annotations": ro,
        },
    ])
}

fn arg_str(a: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| a[*k].as_str()).map(str::to_string)
}

fn arg_num(a: &Value, key: &str, default: usize) -> usize {
    let v = &a[key];
    let n = v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(-1.0);
    if n.is_finite() && n >= 0.5 { (n.round() as usize).min(1000) } else { default }
}

fn cursor(a: &Value) -> Result<usize, String> {
    match &a["cursor"] {
        Value::Null => Ok(0),
        Value::Number(n) => Ok(n.as_u64().unwrap_or(0) as usize),
        Value::String(s) if s.trim().is_empty() => Ok(0),
        Value::String(s) => s.trim().parse().map_err(|_| format!("bad cursor {s}")),
        other => Err(format!("bad cursor {other}")),
    }
}

fn patterns(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().filter_map(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).collect(),
        Value::String(s) if s.trim_start().starts_with('[') => serde_json::from_str(s).unwrap_or_else(|_| vec![s.clone()]),
        Value::String(s) if !s.is_empty() => vec![s.clone()],
        _ => Vec::new(),
    }
}

#[derive(Default)]
struct Parts {
    words: Vec<String>,
    filters: Vec<String>,
    globs: Vec<String>,
}

const FILTERS: &[&str] = &["ext", "type", "kind", "in", "size", "mtime", "modified", "re", "path", "git", "limit"];
const GREP_FILTERS: &[&str] = &["ext", "in", "git", "size", "mtime"];

fn split(query: &str, grep: bool) -> Parts {
    let mut p = Parts::default();
    let words = words(query);
    let texts = words.iter().filter(|w| !is_constraint(w, grep)).count();
    for w in words {
        let (neg, body) = match w.strip_prefix('!') {
            Some(b) if !b.is_empty() => (true, b),
            _ => (false, w.as_str()),
        };
        if let Some((k, v)) = body.split_once(':')
            && !v.is_empty()
            && (if grep { GREP_FILTERS } else { FILTERS }).contains(&k)
        {
            p.filters.push(w.clone());
        } else if let Some(exts) = ext_glob(body) {
            if neg { p.globs.push(w.clone()) } else { p.filters.push(format!("ext:{exts}")) }
        } else if is_glob(body) || (body.len() > 1 && body.ends_with('/')) {
            p.globs.push(w.clone());
        } else if neg {
            p.globs.push(format!("!*{body}*/"));
        } else if grep && texts > 0 && is_file(body) {
            p.globs.push(body.to_string());
        } else {
            p.words.push(w);
        }
    }
    p
}

fn is_constraint(w: &str, grep: bool) -> bool {
    let b = w.strip_prefix('!').unwrap_or(w);
    w.starts_with('!')
        || b.split_once(':').is_some_and(|(k, v)| !v.is_empty() && (if grep { GREP_FILTERS } else { FILTERS }).contains(&k))
        || ext_glob(b).is_some()
        || is_glob(b)
        || (b.len() > 1 && b.ends_with('/'))
        || (grep && is_file(b))
}

fn words(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn ext_glob(w: &str) -> Option<String> {
    let rest = w.strip_prefix("**/").unwrap_or(w).strip_prefix("*.")?;
    let list = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')).unwrap_or(rest);
    let ok = !list.is_empty() && list.split(',').all(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    ok.then(|| list.to_string())
}

fn is_glob(w: &str) -> bool {
    let wild = w.contains(['*', '?', '[']) || (w.contains('{') && w.contains(','));
    wild && (w.contains('/')
        || (w.contains('{') && w.contains('}'))
        || (w.len() > 2 && w.starts_with('*') && w.ends_with('*') && !w[1..w.len() - 1].contains('*')))
}

fn is_file(w: &str) -> bool {
    let name = w.rsplit('/').next().unwrap_or(w);
    match name.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && (1..=10).contains(&ext.len())
                && ext.starts_with(|c: char| c.is_ascii_alphabetic())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && w.chars().all(|c| c.is_alphanumeric() || "._-/@+".contains(c))
        }
        None => false,
    }
}

fn is_file_glob(g: &str) -> bool {
    !g.starts_with('!') && !is_glob(g) && is_file(g)
}

fn looks_like_regex(p: &str) -> bool {
    let escapes = ["\\b", "\\s", "\\d", "\\w", "\\S", "\\W", "\\D", "\\(", "\\[", "\\.", "\\{", "(?"];
    let alternation = p.contains('|') && !p.contains("||") && p.split('|').all(|a| !a.is_empty() && !a.starts_with(' ') && !a.ends_with(' '));
    let class = p.find('[').is_some_and(|i| p[i..].find(']').is_some_and(|j| p[i..i + j].contains('-')));
    escapes.iter().any(|e| p.contains(e))
        || [".*", ".+", ".?"].iter().any(|e| p.contains(e))
        || alternation
        || class
        || (p.len() > 1 && p.starts_with('^'))
        || (p.len() > 1 && p.ends_with('$') && !p.ends_with("\\$"))
}

fn def_name(p: &str) -> Option<&str> {
    if is_ident(p) {
        return Some(p);
    }
    let (kw, name) = p.split_once(' ')?;
    (DEF_WORDS.contains(&kw) && is_ident(name)).then_some(name)
}

fn is_ident(p: &str) -> bool {
    p.len() >= 3 && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && p.chars().any(|c| c.is_ascii_alphabetic())
}

fn spelling(r: &Value, re: &str) -> Option<String> {
    let re = regex::Regex::new(re).ok()?;
    let mut n: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for m in files(r).iter().flat_map(|f| f["matches"].as_array().into_iter().flatten()) {
        for w in re.find_iter(m["text"].as_str().unwrap_or("")) {
            *n.entry(w.as_str()).or_default() += 1;
        }
    }
    n.into_iter().max_by_key(|&(w, c)| (c, std::cmp::Reverse(w))).map(|(w, _)| w.to_string()).filter(|w| is_ident(w))
}

fn variants(id: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut prev = '_';
    for c in id.chars() {
        if c == '_' || c == '-' {
            prev = '_';
            continue;
        }
        let boundary = prev == '_' || (c.is_ascii_uppercase() && !prev.is_ascii_uppercase()) || (c.is_ascii_digit() != prev.is_ascii_digit());
        if boundary || parts.is_empty() {
            parts.push(String::new());
        }
        parts.last_mut().unwrap().push(c);
        prev = c;
    }
    format!("(?i){}", parts.iter().map(|p| regex::escape(p)).collect::<Vec<_>>().join("[_-]?"))
}

const DEF_WORDS: &[&str] = &[
    "fn",
    "func",
    "function",
    "def",
    "class",
    "struct",
    "enum",
    "trait",
    "interface",
    "type",
    "impl",
    "module",
    "mod",
    "object",
    "protocol",
    "extension",
    "union",
    "typedef",
    "namespace",
    "record",
    "actor",
    "macro_rules!",
    "#define",
];
const MODIFIERS: &[&str] =
    &["pub", "export", "default", "async", "abstract", "unsafe", "static", "protected", "private", "public", "final", "inline", "extern", "const"];

fn is_def(line: &str, name: Option<&str>) -> bool {
    let mut s = line.trim_start();
    loop {
        if let Some(r) = s.strip_prefix("pub(").and_then(|r| r.split_once(')')) {
            s = r.1.trim_start();
            continue;
        }
        match MODIFIERS.iter().find(|m| s.strip_prefix(**m).is_some_and(|r| r.starts_with([' ', '\t']))) {
            Some(m) => s = s[m.len()..].trim_start(),
            None => break,
        }
    }
    let kw = DEF_WORDS.iter().find(|k| s.strip_prefix(**k).is_some_and(|r| r.starts_with([' ', '\t', '<', '(']) || r.is_empty()));
    match (kw, name) {
        (Some(k), Some(n)) => {
            let rest = s[k.len()..].trim_start_matches(|c: char| c.is_whitespace() || c == '*' || c == '&');
            let rest = rest.strip_prefix("mut ").unwrap_or(rest);
            let Some(tail) = rest.strip_prefix(n).filter(|t| !t.starts_with(|c: char| c.is_alphanumeric() || c == '_')) else {
                return matches!(*k, "struct" | "union" | "enum") && c_fn(line, n);
            };
            let tail = tail.trim();

            match *k {
                "struct" | "union" | "enum" | "class" => {
                    let tail = tail.strip_prefix('<').map_or(tail, |t| {
                        let mut depth = 1;
                        let end = t.find(|c| {
                            depth += (c == '<') as i32 - (c == '>') as i32;
                            depth == 0
                        });
                        end.map_or("", |e| t[e + 1..].trim_start())
                    });
                    tail.is_empty() || tail.starts_with(['{', ':', '(']) || ["where", "extends", "implements"].iter().any(|w| tail.starts_with(w))
                }
                _ => !tail.ends_with(';') || tail.contains(['=', '{']),
            }
        }
        (Some(_), None) => true,
        (None, Some(n)) => c_fn(line, n),
        (None, None) => false,
    }
}

fn c_fn(line: &str, n: &str) -> bool {
    let t = line.trim_end();
    !line.starts_with([' ', '\t'])
        && !t.ends_with(';')
        && !t.starts_with("extern")
        && t.find(n).is_some_and(|i| {
            let (before, after) = (&t[..i], t[i + n.len()..].trim_start());
            after.starts_with('(') && !before.contains(['=', '(']) && (before.is_empty() || before.ends_with([' ', '*']))
        })
}

fn opens_body(line: &str) -> bool {
    line.trim_end().ends_with(['{', '\\', ':', '(', ',', ')'])
}

fn aux(path: &str) -> bool {
    path.split('/').any(|c| AUX.contains(&c))
}

const AUX: &[&str] = &[
    "tools",
    "test",
    "tests",
    "testing",
    "selftests",
    "testdata",
    "fixtures",
    "samples",
    "examples",
    "Documentation",
    "docs",
    "scripts",
    "benches",
    "vendor",
    "third_party",
];

fn def_rank(path: &str, line: &str, name: &str) -> (bool, bool, usize, bool) {
    let missing = name.split('_').filter(|w| w.len() >= 3 && !path.contains(w)).count();
    (aux(path), ["static ", "impl"].iter().any(|w| line.trim_start().starts_with(w)), missing, line.contains("{}"))
}

fn prototype(path: &str, m: &Value) -> bool {
    let text = m["text"].as_str().unwrap_or("").trim_end();
    if !text.ends_with([',', '(']) || !path.rsplit_once('.').is_some_and(|(_, e)| ["c", "h", "cc", "cpp", "hpp", "m", "mm"].contains(&e)) {
        return false;
    }
    let n = m["line"].as_u64().unwrap_or(0) as usize;
    for l in read_text(path).lines().skip(n).take(16) {
        if l.contains('{') {
            return false;
        }
        if l.trim_end().ends_with(';') {
            return true;
        }
    }
    false
}

fn is_import(line: &str) -> bool {
    let s = line.trim_start();
    ["import ", "use ", "from ", "#include", "require(", "@import"].iter().any(|p| s.starts_with(p))
}

fn clip(line: &str, pat: &str, max: usize) -> String {
    let t = line.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let at = if pat.is_empty() { None } else { t.to_lowercase().find(&pat.to_lowercase()) };
    let start = at.map_or(0, |i| i.saturating_sub(max / 3));
    let start = (0..=start).rev().find(|&i| t.is_char_boundary(i)).unwrap_or(0);
    let end = t[start..].char_indices().nth(max).map_or(t.len(), |(i, _)| start + i);
    format!("{}{}{}", if start > 0 { "…" } else { "" }, &t[start..end], if end < t.len() { "…" } else { "" })
}

fn read_text(path: &str) -> String {
    match std::fs::read(path) {
        Ok(b) if b.len() <= 8 << 20 => String::from_utf8(b).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
        _ => String::new(),
    }
}

fn within(path: &str, dir: &str) -> bool {
    path.strip_prefix(dir).is_some_and(|r| r.is_empty() || r.starts_with('/'))
}

fn repo_root(dir: &str, home: &str) -> Option<String> {
    let mut d = Path::new(dir);
    loop {
        if d.join(".git").exists() {
            let s = d.to_string_lossy();
            return (s != home && s != "/").then(|| s.into_owned());
        }
        d = d.parent()?;
    }
}

fn file_uri(u: &str) -> Option<String> {
    let p = u.strip_prefix("file://")?;
    let p = p.strip_prefix("localhost").unwrap_or(p);
    let b = p.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(h) = p.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(h);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    let s = String::from_utf8(out).ok()?;
    Some(s.trim_end_matches('/').to_string()).filter(|s| s.starts_with('/'))
}
