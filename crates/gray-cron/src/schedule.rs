use chrono::{DateTime, Utc};
use std::str::FromStr;

/// Parsed schedule
/// Kinds: Cron (recurring), Interval (recurring every X), Once (one-shot at time)
#[derive(Debug, Clone)]
pub enum Schedule {
    Cron(cron::Schedule),
    Interval(std::time::Duration),
    Once(DateTime<Utc>),
}

impl Schedule {
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Schedule::Cron(s) => s.after(&after).next(),
            Schedule::Interval(d) => Some(after + chrono::Duration::from_std(*d).ok()?),
            Schedule::Once(at) => {
                if *at > after {
                    Some(*at)
                } else {
                    // One-shot grace (120s): keep due for 2m past time
                    let grace = chrono::Duration::seconds(120);
                    if *at + grace >= after {
                        Some(*at)
                    } else {
                        None
                    }
                }
            }
        }
    }

    /// Human display
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
            Schedule::Once(at) => format!("once at {}", at.format("%Y-%m-%d %H:%M UTC")),
        }
    }

    pub fn is_once(&self) -> bool {
        matches!(self, Schedule::Once(_))
    }
}

/// Parse a schedule string — bare "10m" stays Interval for backwards compat
///
/// Supports:
/// - cron: "0 * * * *", "0 9 * * *" (5 fields, prepends sec 0)
/// - interval: "every 10m", "every 1h" (recurring)
/// - once: "in 10m", "once in 30m", "2026-02-03T14:00", "2026-02-03 14:30" (one-shot)
/// - bare "10m" → Interval (backwards compat)
pub fn parse_schedule(s: &str) -> anyhow::Result<Schedule> {
    let trimmed = s.trim();
    let lower = trimmed.to_lowercase();

    // "in 10m" / "once in 10m" → Once (AI self-scheduling)
    if let Some(rest) = lower.strip_prefix("once in ").or_else(|| lower.strip_prefix("in ")) {
        let dur = parse_duration(rest.trim())?;
        if dur.as_secs() == 0 {
            anyhow::bail!("interval must be > 0");
        }
        let at = Utc::now() + chrono::Duration::from_std(dur).unwrap_or(chrono::Duration::seconds(0));
        return Ok(Schedule::Once(at));
    }
    if lower.starts_with("once at ") {
        let ts = trimmed[8..].trim();
        if let Some(dt) = parse_timestamp(ts) {
            return Ok(Schedule::Once(dt));
        }
    }

    // ISO timestamp like "2026-02-03T14:00" or "2026-02-03 14:30" → Once
    if looks_like_timestamp(trimmed) {
        if let Some(dt) = parse_timestamp(trimmed) {
            return Ok(Schedule::Once(dt));
        }
    }

    // Try cron first if it looks like 5 fields (first 5 fields digit/*-,/)
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if (5..=6).contains(&parts.len())
        && parts[..5].iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit() || "*-/,".contains(c)))
    {
        let cron_candidate = if parts.len() == 5 {
            format!("0 {trimmed}")
        } else {
            trimmed.to_string()
        };
        if let Ok(sched) = cron::Schedule::from_str(&cron_candidate) {
            return Ok(Schedule::Cron(sched));
        }
    }

    // Interval: "every <duration>"
    if let Some(rest) = lower.strip_prefix("every ") {
        let dur = parse_duration(rest.trim())?;
        if dur.as_secs() == 0 {
            anyhow::bail!("interval must be > 0");
        }
        return Ok(Schedule::Interval(dur));
    }
    // Bare duration like "10m" → Interval (Gray compat)
    if let Ok(dur) = parse_duration(trimmed) {
        if dur.as_secs() > 0 {
            return Ok(Schedule::Interval(dur));
        }
    }
    // Fallback cron try (5-field gets a prepended sec 0)
    // One attempt, not three — the extra retries re-ran the same parse.
    let candidates = [
        (trimmed.split_whitespace().count() == 5).then(|| format!("0 {trimmed}")),
        Some(trimmed.to_string()),
    ];
    for c in candidates.into_iter().flatten() {
        if let Ok(sched) = cron::Schedule::from_str(&c) {
            return Ok(Schedule::Cron(sched));
        }
    }
    anyhow::bail!(
        "invalid schedule '{}' — use cron '0 * * * *', 'every 10m', 'in 10m', or '2026-02-03T14:00'",
        s
    )
}

fn looks_like_timestamp(s: &str) -> bool {
    s.contains('T')
        || s.get(..10)
            .is_some_and(|p| chrono::NaiveDate::parse_from_str(p, "%Y-%m-%d").is_ok())
}

fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    use chrono::{NaiveDate, NaiveDateTime, TimeZone};
    let s = s.replace('Z', "+00:00");
    if let Ok(dt) = DateTime::parse_from_rfc3339(&s) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(&s, fmt) {
            return Some(Utc.from_utc_datetime(&naive));
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d") {
        if let Some(ndt) = d.and_hms_opt(0, 0, 0) {
            return Some(Utc.from_utc_datetime(&ndt));
        }
    }
    None
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
    // Next run is just the parsed schedule's next fire after `from`.
    parse_schedule(schedule_str).ok()?.next_after(from)
}

/// Compute next run from stored schedule string + last_run
/// Used by store.rs for updating next_run after a fire
pub fn compute_next_run_after(schedule_str: &str, last_run: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    let sched = parse_schedule(schedule_str).ok()?;
    match &sched {
        Schedule::Once(at) => {
            // Once only fires once — if already run, no next
            if last_run.is_some() {
                return None;
            }
            // Check grace: if at is past + 120s, don't schedule
            let now = Utc::now();
            if *at < now - chrono::Duration::seconds(120) {
                return None;
            }
            Some(*at)
        }
        _ => sched.next_after(last_run.unwrap_or_else(Utc::now)),
    }
}

/// Human shorthand: "check inbox every 30m" → (schedule, prompt)
/// Tries trailing " every ..." or " in ..." or cron at end, else prompt+schedule split.
pub fn split_human_input(input: &str) -> Option<(String, String)> {
    let trimmed = input.trim();
    // Try " ... every 30m" at end
    for marker in [" every ", " in ", " at "] {
        if let Some(idx) = trimmed.to_lowercase().rfind(marker) {
            let prompt = trimmed[..idx].trim();
            let sched_str = trimmed[idx + 1..].trim(); // keep "every ..." without leading space
            // For "in ", need to keep "in ..."
            let sched_candidate = if marker == " in " {
                format!("in {}", &trimmed[idx + 4..].trim())
            } else if marker == " at " {
                trimmed[idx + 1..].trim().to_string()
            } else {
                sched_str.to_string()
            };
            if parse_schedule(&sched_candidate).is_ok() && !prompt.is_empty() {
                return Some((sched_candidate, prompt.to_string()));
            }
        }
    }
    // Try last 5 tokens as cron
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() >= 6 {
        let cron_candidate = parts[parts.len() - 5..].join(" ");
        if parse_schedule(&cron_candidate).is_ok() {
            let prompt = parts[..parts.len() - 5].join(" ");
            if !prompt.is_empty() {
                return Some((cron_candidate, prompt));
            }
        }
    }
    // Try whole input as schedule? then prompt empty → need prompt
    None
}

