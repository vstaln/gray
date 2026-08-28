//! Shared path validation helpers for tool implementations.
//! Port of `tools/path_security.py` (43 lines) — 1:1 behavior.
//!
//! Extracts the `resolve() + relative_to()` and `..` traversal check
//! patterns previously duplicated across skill_manager_tool, skills_tool,
//! skills_hub, cronjob_tools, and credential_files.

use std::path::{Component, Path, PathBuf};

/// Ensure `path` resolves to a location within `root`.
///
/// Returns an error message string if validation fails, or `None` if the
/// path is safe. Uses symlink-aware resolution (mirrors Python
/// `Path.resolve()`) and `relative_to` semantics (`starts_with`).
///
/// Usage:
///
/// ```ignore
/// if let Some(err) = validate_within_dir(&user_path, &allowed_root) {
///     return tool_error(err);
/// }
/// ```
pub fn validate_within_dir(path: &Path, root: &Path) -> Option<String> {
    let resolved = resolve_for_check(path);
    let root_resolved = resolve_for_check(root);
    match (&resolved, &root_resolved) {
        (Ok(r), Ok(root_r)) => {
            if r.starts_with(root_r) {
                None
            } else {
                // Mirrors Python `except (ValueError, OSError) as exc: return f"Path escapes ...: {exc}"`
                // where `relative_to` raises ValueError with "<path> is not in the subpath of <root>".
                Some(format!(
                    "Path escapes allowed directory: {} is not in the subpath of {}",
                    r.display(),
                    root_r.display()
                ))
            }
        }
        (Err(e), _) | (_, Err(e)) => Some(format!("Path escapes allowed directory: {e}")),
    }
}

/// Return true if `path_str` contains `..` traversal components.
///
/// Quick check for obvious traversal attempts before doing full resolution.
/// Mirrors Python `Path(path_str).parts` containment check.
pub fn has_traversal_component(path_str: &str) -> bool {
    Path::new(path_str)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
}

// ---------------------------------------------------------------------------
// Internal: resolve helpers (mirrors Python Path.resolve(strict=False))
// ---------------------------------------------------------------------------

fn resolve_for_check(p: &Path) -> Result<PathBuf, std::io::Error> {
    // Try to canonicalize (follows symlinks, requires existence).
    // If the path does not exist, fall back to lexical absolute normalization
    // which mirrors Python's `resolve(strict=False)` for non-existent leaves.
    match p.canonicalize() {
        Ok(canon) => Ok(canon),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(lexical_absolute(p))
        }
        Err(e) => {
            // For non-NotFound errors (permission, loop, etc.) try to resolve
            // the longest existing ancestor canonically, then append remainder
            // lexically. If that still fails, surface the IO error as escape.
            if let Some(fallback) = try_resolve_prefix(p) {
                Ok(fallback)
            } else {
                Err(e)
            }
        }
    }
}

/// Lexical absolute + dot-normalization without touching the filesystem.
///
/// Makes `p` absolute via current_dir (if relative) then collapses `.` and
/// `..` purely lexically. Does not resolve symlinks (fallback when fs
/// canonicalization is unavailable).
fn lexical_absolute(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(p)
    } else {
        PathBuf::from("/").join(p)
    };
    normalize_lexically(&abs)
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => out.push(n),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(Component::RootDir.as_os_str());
    }
    out
}

/// Try to canonicalize the longest existing prefix, then append remaining
/// components lexically. Returns None if no prefix can be canonicalized.
fn try_resolve_prefix(p: &Path) -> Option<PathBuf> {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(p)
    } else {
        return None;
    };

    // Walk ancestors from longest to shortest to find one that canonicalizes.
    let mut current = abs.as_path();
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();

    loop {
        match current.canonicalize() {
            Ok(canon) => {
                let mut out = canon;
                // remainder was collected in reverse, push in order
                for part in remainder.iter().rev() {
                    out.push(part);
                }
                return Some(normalize_lexically(&out));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // pop one component and continue
                if let Some(parent) = current.parent() {
                    if let Some(file_name) = current.file_name() {
                        remainder.push(file_name.to_os_string());
                    } else {
                        // root or empty
                        break;
                    }
                    current = parent;
                    if current.as_os_str().is_empty() {
                        break;
                    }
                } else {
                    break;
                }
            }
            Err(_) => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn has_traversal_detects_parent_dir() {
        assert!(has_traversal_component("../etc/passwd"));
        assert!(has_traversal_component("a/b/../c"));
        assert!(has_traversal_component("a/../../b"));
        assert!(!has_traversal_component("a/b/c"));
        assert!(!has_traversal_component("a..b/c"));
        assert!(!has_traversal_component(""));
        assert!(!has_traversal_component("a/b/c.."));
    }

    #[test]
    fn validate_within_dir_allows_inside() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("hermes_path_security_test_root_allow");
        let _ = fs::create_dir_all(&root);
        let inside = root.join("sub/file.txt");
        // lexical check even if file doesn't exist (fallback)
        assert_eq!(validate_within_dir(&inside, &root), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn validate_within_dir_rejects_outside() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("hermes_path_security_test_root_deny");
        let _ = fs::create_dir_all(&root);
        let outside = tmp.join("hermes_path_security_test_root_deny_outside");
        let traversal = root.join("../hermes_path_security_test_root_deny_outside");
        let err = validate_within_dir(&traversal, &root);
        assert!(err.is_some());
        assert!(err.unwrap().starts_with("Path escapes allowed directory:"));
        let err2 = validate_within_dir(&outside, &root);
        assert!(err2.is_some());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn validate_within_dir_rejects_traversal_string_resolved() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("hermes_path_security_test_root_traversal");
        let _ = fs::create_dir_all(&root);
        // Path that lexically escapes via ..
        let escaped = root.join("a/../../etc/passwd");
        assert!(validate_within_dir(&escaped, &root).is_some());
        let _ = fs::remove_dir_all(&root);
    }
}
