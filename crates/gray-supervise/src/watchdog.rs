//! Startup/shutdown budgets. Boot must mark ready within the timeout or exit 75.
pub const SHUTDOWN_DRAIN_SECS: u64 = 30;

/// `GRAY_STARTUP_TIMEOUT_SECS`, default 120, clamped to min 30.
pub fn startup_timeout_secs() -> u64 {
    std::env::var("GRAY_STARTUP_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(120)
        .max(30)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_match_spec() {
        assert_eq!(SHUTDOWN_DRAIN_SECS, 30);
        assert_eq!(startup_timeout_secs(), 120);
    }
}
