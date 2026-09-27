//! `gray find` / `gray grep` — CLI half of the search command.
//!
//! All the work lives in `gray_tools::search_cmd`, because the bash tool
//! claims the same two commands and cannot reach into this crate. These are
//! the two-line wrappers that print what a human asked for; the agent path
//! gets the identical text as a tool result.

use std::path::PathBuf;

use gray_core::agent::ToolContext;
use gray_tools::search_cmd::{self, SearchArgs};

/// `gray find PATTERN [PATH]` — resolve the path against the cwd, then print.
pub async fn run_find(
    pattern: &str,
    path: Option<&str>,
    limit: Option<usize>,
) -> anyhow::Result<()> {
    let ctx = ToolContext::default();
    let args = SearchArgs {
        pattern: pattern.to_string(),
        path: resolve(path, &ctx.cwd),
        limit,
        ..Default::default()
    };
    println!("{}", search_cmd::find(&args, &ctx).await);
    Ok(())
}

/// `gray grep PATTERN [PATH]` and every flag it takes.
#[allow(clippy::too_many_arguments)]
pub async fn run_grep(
    pattern: &str,
    path: Option<&str>,
    limit: Option<usize>,
    glob: Option<&str>,
    ignore_case: bool,
    literal: bool,
    context: Option<usize>,
) -> anyhow::Result<()> {
    let ctx = ToolContext::default();
    let args = SearchArgs {
        pattern: pattern.to_string(),
        path: resolve(path, &ctx.cwd),
        limit,
        glob: glob.map(str::to_string),
        ignore_case,
        literal,
        context,
    };
    println!("{}", search_cmd::grep(&args, &ctx).await);
    Ok(())
}

fn resolve(path: Option<&str>, cwd: &std::path::Path) -> Option<PathBuf> {
    path.map(|p| gray_core::tool_out::resolve_path(cwd, p))
}
