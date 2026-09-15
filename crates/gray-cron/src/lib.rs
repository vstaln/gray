//! gray-cron: recurring/one-shot job scheduling for the gray daemon.
//!
//! Task 1: schedule expressions (pure math). Job store + ticker land in
//! Tasks 2-3. Time is UTC everywhere, comparisons in epoch seconds.

pub mod schedule;
pub mod store;

pub use schedule::Schedule;
pub use store::TICKER_STALE_SECS;
pub use store::{
    CronHealth, CronJob, CronStore, Deliver, OverdueJob, RunStatus, TickStamp, now_secs,
};
