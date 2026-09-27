//! `gray find` / `gray grep` — the search index as a *command*, not a
//! tool.
//!
//! The index used to sit behind the `find`/`grep` tools, which quietly
//! changed what two tools return depending on the directory, the pattern and
//! a decade of glob semantics. That is the wrong shape for a harness whose
//! whole claim is that the model drives one `bash`: searching is already a
//! shell thing (`fd`, `rg`, `grep`), so the index belongs beside them as a
//! command the model reaches for on purpose.
//!
//! The contract is therefore the simplest one that can exist: **whatever
//! `fd`/`rg` would have answered, this answers.** The index is a fast path in
//! front of the existing lanes, never a different answer — when it is not
//! usable (no index in this process yet, a non-git root, `ignoreCase`, a
//! depth-anchored glob, an invalid pattern, a file target) the call falls
//! through to the very same `FindTool`/`GrepTool` the model could have run
//! itself, so the bytes are identical either way.
//!
//! "Not usable yet" is the load-bearing part, and it is about *cost*, not
//! capability. The index lives in process memory, so a fresh process holding
//! none would pay a full scan to answer a question `fd` answers in ~20ms —
//! measured at 1.4s against 20ms on a 20k-file repo. So the first search in a
//! process goes to `fd`/`rg` and starts the index in the background; every
//! search after that is served warm. The index therefore has to *earn* each
//! call, which is the only arrangement where it is ever the right answer.
//!
//! Both entry points share this module: the `gray find`/`gray grep` CLI verbs
//! and the bash claim that answers them without depending on `gray` being on
//! the child's PATH.

use std::path::{Path, PathBuf};

use gray_core::agent::{Tool, ToolContext};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// What one search call needs. Mirrors the `find`/`grep` tool arguments so a
/// model that learned the flags from either side is not wrong.
#[derive(Debug, Default, Clone)]
pub struct SearchArgs {
    pub pattern: String,
    /// Search root. Defaults to the cwd, like the tools.
    pub path: Option<PathBuf>,
    pub limit: Option<usize>,
    /// `grep` only: basename glob filter, passed through untouched.
    pub glob: Option<String>,
    pub ignore_case: bool,
    pub literal: bool,
    pub context: Option<usize>,
}

impl SearchArgs {
    fn root(&self, cwd: &Path) -> PathBuf {
        self.path.clone().unwrap_or_else(|| cwd.to_path_buf())
    }
}

/// `gray find PATTERN [PATH]` — file names matching a glob, off the
/// process-wide pool.
pub async fn find(args: &SearchArgs, ctx: &ToolContext) -> String {
    find_with_pool(args, ctx, crate::search_index::global_pool().clone()).await
}

/// [`find`] against a caller-chosen pool. The global pool roots its frecency
/// databases under `gray_home()`; a test (or a future embedder) passes its own.
pub async fn find_with_pool(
    args: &SearchArgs,
    ctx: &ToolContext,
    pool: std::sync::Arc<crate::search_index::SearchPool>,
) -> String {
    let root = args.root(&ctx.cwd);
    let limit = args.limit.unwrap_or(100);
    if !ctx.cancel.is_cancelled() && pool.warm_picker(&root).is_some() {
        let (dir, pat) = (root.clone(), args.pattern.clone());
        let lane = pool.clone();
        let hits = tokio::select! {
            out = spawn_index(move || lane.indexed_glob(&dir, &pat, limit)) => out,
            _ = ctx.cancel.cancelled() => None,
        };
        if let Some(hits) = hits {
            return render_find(&hits, limit);
        }
    }
    // Nothing warm: answer from the tool, and let the next search in this
    // process find an index waiting for it — unless the caller cancelled, in
    // which case it asked for no work at all.
    if !ctx.cancel.is_cancelled() {
        pool.warm_in_background(&root);
    }
    tool_text(
        &crate::FindTool,
        ctx,
        json!({
            "pattern": args.pattern,
            "path": root.to_string_lossy(),
            "limit": limit,
        }),
    )
    .await
}

/// `gray grep PATTERN [PATH]` — file contents matching a pattern, off the
/// process-wide pool.
pub async fn grep(args: &SearchArgs, ctx: &ToolContext) -> String {
    grep_with_pool(args, ctx, crate::search_index::global_pool().clone()).await
}

/// [`grep`] against a caller-chosen pool.
pub async fn grep_with_pool(
    args: &SearchArgs,
    ctx: &ToolContext,
    pool: std::sync::Arc<crate::search_index::SearchPool>,
) -> String {
    let root = args.root(&ctx.cwd);
    let limit = args.limit.unwrap_or(100);
    let context = args.context.unwrap_or(0);
    if !ctx.cancel.is_cancelled() && pool.warm_picker(&root).is_some() {
        let (dir, pat, g) = (root.clone(), args.pattern.clone(), args.glob.clone());
        let (ic, lit, lim, ctxn) = (args.ignore_case, args.literal, limit, context);
        let lane = pool.clone();
        let found = tokio::select! {
            out = spawn_index(move || {
                // The query is already bounded by `limit`; the token only
                // exists because `indexed_grep` takes one.
                let cancel = CancellationToken::new();
                lane.indexed_grep(&dir, &pat, g.as_deref(), ic, lit, lim, ctxn, &cancel)
            }) => out,
            _ = ctx.cancel.cancelled() => None,
        };
        if let Some(found) = found {
            return crate::grep::render_matches(&root, &found.hits, limit, found.limit_reached);
        }
    }
    if !ctx.cancel.is_cancelled() {
        pool.warm_in_background(&root);
    }
    let mut call = json!({
        "pattern": args.pattern,
        "path": root.to_string_lossy(),
        "limit": limit,
    });
    if context > 0 {
        call["context"] = json!(context);
    }
    if let Some(g) = &args.glob {
        call["glob"] = json!(g);
    }
    if args.ignore_case {
        call["ignoreCase"] = json!(true);
    }
    if args.literal {
        call["literal"] = json!(true);
    }
    tool_text(&crate::GrepTool, ctx, call).await
}

/// The index is synchronous CPU work, so it belongs off the async runtime.
/// The spawned task keeps indexing in the background after a cancel, which is
/// exactly what the next call wants.
async fn spawn_index<F, T>(f: F) -> Option<T>
where
    F: FnOnce() -> Option<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).await.ok().flatten()
}

/// Index hits as the `find` tool would print them: newline-joined relative
/// paths plus the same notices, so the two lanes are indistinguishable.
fn render_find(hits: &[String], limit: usize) -> String {
    use crate::MAX_BYTES;
    use crate::truncate::{append_notices, format_size, truncate_head};
    if hits.is_empty() {
        return "No files found matching pattern".to_string();
    }
    let mut notices: Vec<String> = Vec::new();
    if hits.len() >= limit {
        notices.push(format!(
            "{limit} results limit reached. Use limit={} for more, or refine pattern",
            limit * 2
        ));
    }
    let trunc = truncate_head(&hits.join("\n"));
    if trunc.truncated {
        notices.push(format!("{} limit reached", format_size(MAX_BYTES)));
    }
    if notices.is_empty() {
        trunc.content
    } else {
        let mut out = trunc.content;
        append_notices(&mut out, &notices);
        out
    }
}

/// The fallback lane: the real tool, so the answer is the tool's answer.
async fn tool_text(tool: &dyn Tool, ctx: &ToolContext, args: Value) -> String {
    tool.execute(ctx, args).await.content
}
