use std::sync::OnceLock;
use std::time::Instant;

use crate::config::Config;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();

pub fn init_process_start() {
    let _ = PROCESS_START.set(Instant::now());
}

pub fn process_start() -> Instant {
    *PROCESS_START.get_or_init(Instant::now)
}

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
