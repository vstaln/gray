//! Shared filesystem roots, without changing process environment. Setting HOME
//! during startup is unsafe once threads exist and makes native and Git tools
//! disagree. Consumers resolve the platform profile directly instead.
use std::path::PathBuf;

pub fn user_home() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        // std uses USERPROFILE, then the Windows profile API. In particular it
        // does not confuse Git Bash's HOME with the native Windows profile.
        std::env::home_dir()
    }
    #[cfg(not(windows))]
    {
        // Preserve the Unix contract: no implicit passwd fallback when a host
        // deliberately removed HOME. Callers already define their error policy.
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

pub fn gray_home() -> Option<PathBuf> {
    std::env::var_os("GRAY_HOME")
        .filter(|v| !v.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| user_home().map(|p| p.join(".gray")))
}
