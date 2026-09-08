//! Schedule expressions (pure math, no I/O).
//!
//! Gray-minimal kinds, no NL weekdays (follow-up):
//! `"in 10m"` / ISO `"2026-02-03T14:00:00Z"` -> Once;
//! `"every 10m"` / bare `"10m"` -> Interval;
//! 5-field `"0 9 * * *"` -> Cron (seconds prepended for the `cron` crate).

use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};

/// Minimum honored period: the daemon ticker runs every 60s.
pub const MIN_INTERVAL_SECS: u64 = 60;
/// One-shot fire grace after its time passes (claim_due retires older).
pub const ONESHOT_GRACE_SECS: i64 = 120;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Schedule {
    Interval { secs: u64 },
    Cron { expr: String },
    Once { at: i64 },
}

/// Half the period clamped to [120s, 7200s] (hermes numbers).
pub fn catchup_grace_secs(period_secs: u64) -> i64 {
    (period_secs / 2).clamp(120, 7200) as i64
}

fn parse_duration_secs(raw: &str) -> anyhow::Result<u64> {
    let s = raw.trim();
    let (num_str, mult) = if let Some(n) = s.strip_suffix('m') {
        (n, 60u64)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3600u64)
    } else if let Some(n) = s.strip_suffix('d') {
        (n, 86400u64)
    } else {
        anyhow::bail!("bad duration {:?}: want <n>m/h/d (e.g. 10m, 1h, 7d)", s);
    };
    let n: u64 = num_str
        .parse()
        .map_err(|_| anyhow::anyhow!("bad duration {:?}: number expected", s))?;
    let secs = n
        .checked_mul(mult)
        .ok_or_else(|| anyhow::anyhow!("bad duration {:?}: overflow", s))?;
    if secs < MIN_INTERVAL_SECS {
        anyhow::bail!("interval {}s below 60s ticker resolution", secs);
    }
    Ok(secs)
}

pub fn parse_schedule(input: &str) -> anyhow::Result<Schedule> {
    let s = input.trim();
    if s.is_empty() {
        anyhow::bail!("empty schedule");
    }
    if let Some(rest) = s.strip_prefix("in ").map(str::trim) {
        let secs = parse_duration_secs(rest)?;
        let at = Utc::now()
            .timestamp()
            .checked_add(secs as i64)
            .ok_or_else(|| anyhow::anyhow!("one-shot time overflow"))?;
        return Ok(Schedule::Once { at });
    }
    if let Some(rest) = s.strip_prefix("every ").map(str::trim) {
        return Ok(Schedule::Interval {
            secs: parse_duration_secs(rest)?,
        });
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(Schedule::Once {
            at: dt.with_timezone(&Utc).timestamp(),
        });
    }
    if let Ok(secs) = parse_duration_secs(s) {
        return Ok(Schedule::Interval { secs });
    }
    if s.split_whitespace().count() == 5 {
        cron::Schedule::from_str(&format!("0 {}", s))
            .map_err(|e| anyhow::anyhow!("bad cron {:?}: {}", s, e))?;
        return Ok(Schedule::Cron {
            expr: s.to_string(),
        });
    }
    anyhow::bail!(
        "bad schedule {:?}: want \"in 10m\", RFC3339, \"every 1h\" / \"30m\", or 5-field cron",
        s
    )
}

pub fn next_run(after: i64, s: &Schedule) -> Option<i64> {
    match s {
        Schedule::Interval { secs } => after.checked_add(*secs as i64),
        Schedule::Once { at } => Some(*at),
        Schedule::Cron { expr } => {
            let sched = cron::Schedule::from_str(&format!("0 {}", expr)).ok()?;
            let after_dt = Utc.timestamp_opt(after, 0).single()?;
            sched.after(&after_dt).next().map(|dt| dt.timestamp())
        }
    }
}

#[cfg(test)]
mod tests {
    // UNRUN (cargo test banned under X): run in TTY/CI.
    use super::*;

    #[test]
    fn parse_all_kinds() {
        assert!(matches!(
            parse_schedule("in 10m").unwrap(),
            Schedule::Once { .. }
        ));
        assert!(matches!(
            parse_schedule("every 1h").unwrap(),
            Schedule::Interval { secs: 3600 }
        ));
        assert!(matches!(
            parse_schedule("30m").unwrap(),
            Schedule::Interval { secs: 1800 }
        ));
        assert!(matches!(
            parse_schedule("0 9 * * *").unwrap(),
            Schedule::Cron { .. }
        ));
        assert!(
            parse_schedule("every 30s").is_err(),
            "below ticker resolution"
        );
    }

    #[test]
    fn catchup_window_math() {
        assert_eq!(catchup_grace_secs(3600), 1800); // half period
        assert_eq!(catchup_grace_secs(60), 120); // clamped floor
        assert_eq!(catchup_grace_secs(86400 * 30), 7200); // clamped ceiling
    }
}
