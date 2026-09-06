//! Restart contract (Hermes-compatible): 75 = restart me, 78 = fatal config.
pub const EXIT_CLEAN: i32 = 0;
pub const EXIT_RESTART: i32 = 75;
pub const EXIT_FATAL: i32 = 78;

#[cfg(test)]
mod tests {
    #[test]
    fn restart_and_fatal_codes_match_hermes_contract() {
        assert_eq!(super::EXIT_CLEAN, 0);
        assert_eq!(super::EXIT_RESTART, 75);
        assert_eq!(super::EXIT_FATAL, 78);
    }
}
