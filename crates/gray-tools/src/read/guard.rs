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
    if is_blocklisted(literal)
        || canonical.is_some_and(|c| c != literal && is_blocklisted(c))
    {
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
        return Ok(MetadataDecision::RegularFile);
    }
    #[cfg(not(unix))]
    {
        return Err(refusal(display, "special file"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_message_is_contract_exact() {
        assert_eq!(
            refusal("/dev/zero", "device/FIFO/socket"),
            "read refused: /dev/zero is a device/FIFO/socket; reading it would block. \
             Use bash with a timeout if you really need it."
        );
    }

    #[test]
    fn blocklisted_names_hit() {
        for p in [
            "/dev/stdin",
            "/dev/stdout",
            "/dev/stderr",
            "/dev/fd",
            "/dev/fd/3",
            "/proc/self/fd",
            "/proc/self/fd/0",
            "/proc/1234/fd",
            "/proc/1234/fd/5",
            "/dev/zero",
            "/dev/urandom",
            "/dev/random",
            "/dev/tty",
            "/dev/tty1",
            "/dev/ttyS0",
            // relative literals anchor the same way
            "dev/fd/3",
        ] {
            assert!(is_blocklisted(Path::new(p)), "{p}");
            assert!(check_name(Path::new(p), None, p).is_err(), "{p}");
        }
    }

    #[test]
    fn innocent_names_pass() {
        for p in [
            "zero",               // regular file merely named `zero`
            "/home/u/zero",       // anchored: only a leading `dev` counts
            "/tmp/x.txt",         //
            "/dev",               // dir itself: metadata gate decides
            "/dev/null",          // not in the name list (char device via metadata)
            "/proc/abc/fd/3",     // non-numeric pid is not the fd-dir shape
            "/proc/1234/task",    // proc, but not an fd dir
            "/proc/self/maps",    //
        ] {
            assert!(!is_blocklisted(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn name_hit_needs_no_io() {
        // Path does not exist, no metadata consulted — proves step 2 runs
        // before any open()/stat().
        let missing = Path::new("/dev/fd/999998");
        assert!(!missing.exists());
        assert!(check_name(missing, None, "/dev/fd/999998").is_err());
    }

    #[test]
    fn canonical_hit_when_literal_hides_device() {
        let literal = Path::new("/tmp/link-to-zero");
        assert!(!is_blocklisted(literal));
        assert!(check_name(literal, Some(Path::new("/dev/zero")), "/tmp/link-to-zero").is_err());
    }

    #[test]
    fn metadata_regular_file_and_dir() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "hi").unwrap();
        assert!(matches!(
            check_metadata(&std::fs::metadata(&file).unwrap(), "a.txt").unwrap(),
            MetadataDecision::RegularFile
        ));
        assert!(matches!(
            check_metadata(&std::fs::metadata(dir.path()).unwrap(), "d").unwrap(),
            MetadataDecision::Directory
        ));
    }

    #[cfg(unix)]
    #[test]
    fn metadata_char_device_fifo_socket_refused() {
        // /dev/null: char device via the metadata half (not the name list).
        if Path::new("/dev/null").exists() {
            let err =
                check_metadata(&std::fs::metadata("/dev/null").unwrap(), "/dev/null").unwrap_err();
            assert!(err.contains("is a device;"), "{err}");
        }
        let dir = tempfile::tempdir().unwrap();
        // ponytail: libc is already a dependency; no new crate for one mkfifo.
        let fifo = dir.path().join("f.fifo");
        let cstr = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cstr.as_ptr(), 0o600) }, 0);
        let err =
            check_metadata(&std::fs::metadata(&fifo).unwrap(), "f.fifo").unwrap_err();
        assert!(err.contains("is a FIFO;"), "{err}");

        let sock = dir.path().join("s.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let err = check_metadata(&std::fs::metadata(&sock).unwrap(), "s.sock").unwrap_err();
        assert!(err.contains("is a socket;"), "{err}");
    }
}
