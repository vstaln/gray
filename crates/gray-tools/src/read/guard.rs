//! T2.3 — device path blocklist + non-regular-file refusal.
//!
//! Self-contained unit: std only, no `crate::` imports. Wired by T1.1
//! (`mod guard;` in `read/mod.rs`, called first in `ReadTool::execute`).
//!
//! ```ignore
//! mod guard;
//! ```
//!
//! Intended caller flow in `ReadTool::execute`, before any content I/O:
//!
//! 1. `let literal = resolve_path(&ctx.cwd, &path);` (absolute, no I/O yet)
//! 2. `guard::check_name(&literal, None, &display)?` — refusal, no `open()`.
//! 3. `let canonical = std::fs::canonicalize(&literal).ok();`
//!    (a symlink may hide a device: `/tmp/link` → `/dev/zero`)
//! 4. `guard::check_name(&literal, canonical.as_deref(), &display)?`
//! 5. `symlink_metadata`, then `metadata` (follow links), then
//!    `guard::check_metadata(&meta, &display)?`:
//!    - `Ok(MetadataDecision::Directory)` → the T1.3 directory note
//!      (wording lives in `notices.rs`, owned by T1.3 — not rendered here).
//!    - `Err(msg)` → return with `is_error=true` (an actual refusal).
//!
//! Name matching is anchored at the first path component, so a regular file
//! merely named `zero` is never refused.

use std::fs;
use std::path::{Component, Path};

/// Refusal text (T2.3 contract). `kind` is `device`, `FIFO`, `socket`, or
/// `device/FIFO/socket` for a name-blocklist hit (target type unseen).
pub fn refusal(display: &str, kind: &str) -> String {
    format!(
        "read refused: {display} is a {kind}; \
         reading it would block. Use bash with a timeout if you really need it."
    )
}

/// Lexical components: drops prefix/root/`.`, folds `..` against the
/// already-seen prefix. Enough for blocklist anchoring; the canonical-path
/// check (step 4 above) covers real symlink/`..` escapes.
fn components(path: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in path.components() {
        match c {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(s) => out.push(s.to_string_lossy().into_owned()),
        }
    }
    out
}

/// True when the path names a T2.3 blocklist entry. Checked against both the
/// literal (resolved, pre-I/O) and the canonical path (post-symlink).
/// Spec-exact list — e.g. `/dev/null` is deliberately NOT here (empty read,
/// harmless); it is still refused by [`check_metadata`] as a char device.
pub fn is_blocklisted(path: &Path) -> bool {
    let c = components(path);
    if c.is_empty() {
        return false;
    }
    if c[0] == "dev" {
        return match c.get(1).map(String::as_str) {
            Some("stdin") | Some("stdout") | Some("stderr") | Some("zero") | Some("urandom")
            | Some("random") => true,
            // /dev/fd itself and /dev/fd/*; /dev/tty, /dev/tty1, /dev/ttyS0, …
            Some("fd") => true,
            Some(tty) if tty.starts_with("tty") => true,
            _ => false,
        };
    }
    if c[0] == "proc" {
        return match (c.get(1).map(String::as_str), c.get(2).map(String::as_str)) {
            (Some("self"), Some("fd")) => true, // /proc/self/fd[/...]
            (Some(pid), Some("fd"))
                if !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()) =>
            {
                true // /proc/<pid>/fd[/...]
            }
            _ => false,
        };
    }
    false
}

/// Name gate: no I/O, safe to call before `open()`. `canonical` is the
/// `std::fs::canonicalize` result when it exists (`None` skips that half).
pub fn check_name(literal: &Path, canonical: Option<&Path>, display: &str) -> Result<(), String> {
    if is_blocklisted(literal) || canonical.is_some_and(|c| c != literal && is_blocklisted(c)) {
        return Err(refusal(display, "device/FIFO/socket"));
    }
    Ok(())
}

/// Outcome of [`check_metadata`] for a path that passed [`check_name`].
/// `Directory` is NOT an error here — the caller renders the T1.3 note.
#[derive(Debug)]
pub enum MetadataDecision {
    RegularFile,
    Directory,
}

/// File-type gate over already-fetched metadata (caller stats first, so this
/// performs no I/O either). `Err` is the T2.3 refusal (`is_error=true`).
pub fn check_metadata(meta: &fs::Metadata, display: &str) -> Result<MetadataDecision, String> {
    let ft = meta.file_type();
    if ft.is_dir() {
        return Ok(MetadataDecision::Directory);
    }
    if ft.is_file() {
        return Ok(MetadataDecision::RegularFile);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if ft.is_char_device() || ft.is_block_device() {
            return Err(refusal(display, "device"));
        }
        if ft.is_fifo() {
            return Err(refusal(display, "FIFO"));
        }
        if ft.is_socket() {
            return Err(refusal(display, "socket"));
        }
        Ok(MetadataDecision::RegularFile)
    }
    #[cfg(not(unix))]
    {
        return Err(refusal(display, "special file"));
    }
}

#[path = "guard_tests.rs"]
#[cfg(test)]
mod tests;
