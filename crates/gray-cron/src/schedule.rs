use chrono::{DateTime, Utc};
use std::str::FromStr;

/// Parsed schedule — either a cron expression (5 fields) or a simple interval.
#[derive(Debug, Clone)]
pub enum Schedule {
    Cron(cron::Schedule),
    Interval(std::time::Duration),
}

impl Schedule {
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Schedule::Cron(s) => s.after(&after).next(),
            Schedule::Interval(d) => Some(after + chrono::Duration::from_std(*d).ok()?),
        }
    }

    pub fn display(&self) -> String {
        match self {
            Schedule::Cron(s) => s.to_string(),
            Schedule::Interval(d) => {
                let secs = d.as_secs();
                if secs % 3600 == 0 {
                    format!("every {}h", secs / 3600)
                } else if secs % 60 == 0 {
                    format!("every {}m", secs / 60)
                } else {
                    format!("every {}s", secs)
                }
            }
        }
    }
}

/// Parse a schedule string.
///
/// Supports:
/// - cron: "0 * * * *", "0 0 * * *", "*/5 * * * *"
/// - interval: "every 10m", "every 1h", "every 30s", "every 2h30m" (simple)
pub fn parse_schedule(s: &str) -> anyhow::Result<Schedule> {
    let trimmed = s.trim();
    // Try cron first if it looks like 5 fields (add seconds)
    let cron_candidate = if trimmed.split_whitespace().count() == 5 {
        format!("0 {trimmed}")
    } else {
        trimmed.to_string()
    };
    if let Ok(sched) = cron::Schedule::from_str(&cron_candidate) {
        return Ok(Schedule::Cron(sched));
    }
    // Try interval: "every <duration>"
    if let Some(rest) = trimmed.strip_prefix("every ") {
        let dur = parse_duration(rest.trim())?;
        if dur.as_secs() == 0 {
            anyhow::bail!("interval must be > 0");
        }
        return Ok(Schedule::Interval(dur));
    }
    // Also allow bare duration like "10m"
    if let Ok(dur) = parse_duration(trimmed) {
        if dur.as_secs() > 0 {
            return Ok(Schedule::Interval(dur));
        }
    }
    // Fallback: try cron with seconds prepended if needed
    let with_secs = if trimmed.split_whitespace().count() == 5 {
        format!("0 {trimmed}")
    } else {
        trimmed.to_string()
    };
    if let Ok(sched) = cron::Schedule::from_str(&with_secs) {
        return Ok(Schedule::Cron(sched));
    }
    if let Ok(sched) = cron::Schedule::from_str(trimmed) {
        return Ok(Schedule::Cron(sched));
    }
    anyhow::bail!(
        "invalid schedule '{}' — use cron '0 * * * *' or 'every 10m' / 'every 1h'",
        s
    )
}

fn parse_duration(s: &str) -> anyhow::Result<std::time::Duration> {
    // Parse like "10m", "1h", "30s", "2h30m", "1d"
    let mut total_secs: u64 = 0;
    let mut num_buf = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            num_buf.push(ch);
        } else if ch.is_ascii_whitespace() {
            continue;
        } else {
            let n: u64 = num_buf
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid duration '{}'", s))?;
            num_buf.clear();
            let mult = match ch {
                's' => 1,
                'm' => 60,
                'h' => 3600,
                'd' => 86400,
                _ => anyhow::bail!("unknown duration unit '{}' in '{}'", ch, s),
            };
            total_secs += n * mult;
        }
    }
    if !num_buf.is_empty() {
        let n: u64 = num_buf.parse()?;
        total_secs += n * 60; // bare number defaults to minutes
    }
    Ok(std::time::Duration::from_secs(total_secs))
}

pub fn compute_next_run(schedule_str: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let sched = parse_schedule(schedule_str).ok()?;
    sched.next_after(from)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_cron() {
        let s = parse_schedule("0 * * * *").unwrap();
        assert!(matches!(s, Schedule::Cron(_)));
    }
    #[test]
    fn parses_interval() {
        let s = parse_schedule("every 10m").unwrap();
        assert!(matches!(s, Schedule::Interval(d) if d.as_secs() == 600));
        let s2 = parse_schedule("every 1h").unwrap();
        assert!(matches!(s2, Schedule::Interval(d) if d.as_secs() == 3600));
    }
    #[test]
    fn next_run_interval() {
        let s = parse_schedule("every 10m").unwrap();
        let now = Utc::now();
        let next = s.next_after(now).unwrap();
        assert!(next > now);
    }
}
