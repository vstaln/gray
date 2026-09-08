//! gray-cron: recurring/one-shot job scheduling for the gray daemon.
//!
//! Task 1: schedule expressions (pure math). Job store + ticker land in
//! Tasks 2-3. Time is UTC everywhere, comparisons in epoch seconds.

pub mod schedule;
pub mod store;

pub use schedule::{
    MIN_INTERVAL_SECS, ONESHOT_GRACE_SECS, Schedule, catchup_grace_secs, next_run, parse_schedule,
};
pub use store::{
    Claim, CronJob, CronStore, Deliver, JobState, Origin, RunStatus, atomic_write_json, now_secs,
    reject_lifecycle_shape,
};
