//! shell: blocking bash tool modules.

pub mod contract;
pub mod exit;
pub mod fence;
pub mod kill;
pub mod pump;
pub mod spawn;
pub mod split;
pub mod tools;
pub mod view;

#[cfg(windows)]
mod windows;

/// Absolute Git Bash/sh path on Windows (public for cross-crate callers
/// like the cron pre-script runner; discovery rules live in `windows`).
#[cfg(windows)]
pub use windows::shell_path;
