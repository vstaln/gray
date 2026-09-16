//! Turn caps: mini-swe-agent `AgentConfig` limits (`step_limit`, `cost_limit`,
//! `wall_time`) in gray's minimal shape.
//!
//! Three optional bounds from [`Config`](crate::config::Config) (CLI > env,
//! never persisted): max turns, max spend, max wall-clock. Checked once per
//! prompt turn in the REPL and once in print mode, before any model work.
//! Unpriced models never trip the spend cap (unknown cost = $0 counted, and
//! the check only fires when a priced total actually exceeds the cap).

use std::sync::OnceLock;
use std::time::Instant;

use crate::config::Config;

/// Process start for the wall-clock cap (set once from `main`, first check
/// wins in tests).
static PROCESS_START: OnceLock<Instant> = OnceLock::new();

/// Records process start for [`check_caps`]. Idempotent (first call wins).
pub fn init_process_start() {
    let _ = PROCESS_START.set(Instant::now());
}

/// Wall-clock base (tests can read it; production sets it via [`init_process_start`]).
pub fn process_start() -> Instant {
    *PROCESS_START.get_or_init(Instant::now)
}

/// Stop message when a cap is hit, else `None`. Pure over turn count +
/// session spend + config (plain fields, not `SessionTotals`, so the public
/// surface stays public-safe).
pub fn check_caps(config: &Config, turns: usize, cost: f64) -> Option<String> {
    if let Some(max) = config.max_turns
        && turns >= max as usize
    {
        return Some(format!(
            "max turns {max} reached — stopping (raise with --max-turns N)"
        ));
    }
    if let Some(cap) = config.max_cost_micros {
        let spent_micros = (cost * 1_000_000.0).round() as u64;
        if spent_micros >= cap {
            return Some(format!(
                "max spend ${:.2} reached ({} session) — stopping (raise with --max-cost-usd USD)",
                cap as f64 / 1_000_000.0,
                crate::setup::format_cost(cost),
            ));
        }
    }
    if let Some(max) = config.max_wall_secs {
        let elapsed = process_start().elapsed().as_secs();
        if elapsed >= max {
            return Some(format!(
                "max wall time {max}s reached — stopping (raise with --max-wall-secs SECS)"
            ));
        }
    }
    None
}

#[path = "turn_caps_tests.rs"]
#[cfg(test)]
mod tests;
