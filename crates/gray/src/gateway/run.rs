//! `gray gateway run`: the foreground daemon the supervisor executes.
//!
//! Shutdown is deliberate (hermes drain doctrine): SIGTERM/SIGINT stop the
//! tick loop, but an in-flight fire gets a bounded drain window to land its
//! own report instead of being shot mid-run (its claim would otherwise sit
//! until the 300s claim TTL). Then the socket closes, the pid claim is
//! released, and the state file records why we stopped.

use std::time::Duration;

use crate::config::Config;
#[cfg(unix)]
use crate::gateway::socket;
use crate::gateway::{pid, state};

/// How long an in-flight cron fire may keep running after a stop signal.
/// Bounded: the claim TTL (300s) releases whatever we abandon.
const DRAIN_SECS: u64 = 65;

/// The ticker heartbeat kind every surface reports (`cron list`, `/cron`).
const TICK_KIND: &str = "gateway";

pub async fn run_foreground(config: &Config) -> anyhow::Result<()> {
    let home = crate::setup::gray_home()?;
    let record = pid::claim(&home)?;
    let started_at = record.started_at;
    log::info!(
        "gateway: starting (pid {}, home {})",
        record.pid,
        home.display()
    );
    let _ = state::write(
        &home,
        &state::record(state::STATE_STARTING, None, started_at),
    );

    // Socket first (after the claim, hermes order): liveness answers before
    // the first tick, and a bind failure is loud but non-fatal.
    // Unix-only: on Windows there is no control socket — pid + state files
    // still answer `gateway status`.
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    #[cfg(unix)]
    let socket_task = {
        let socket_home = home.clone();
        tokio::spawn(async move {
            if let Err(e) = socket::serve(socket_home, stop_rx).await {
                log::warn!("gateway: control socket disabled: {e:#}");
            }
        })
    };
    #[cfg(not(unix))]
    let socket_task = {
        let _ = stop_rx;
        tokio::spawn(async move {
            log::warn!("gateway: control socket unavailable on this platform");
        })
    };
    let _ = state::write(
        &home,
        &state::record(state::STATE_RUNNING, None, started_at),
    );

    let store = crate::cron::CronStore::open(home.join("cron"))?;
    let runner = crate::cron_serve::HeadlessRunner {
        config: config.clone(),
    };
    let deliver = crate::cron_serve::SaveLocalDeliver { home: home.clone() };
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut stop_signal = stop_signal();
    let mut exit_reason: &'static str = "stop";

    enum TickEnd {
        Finished(anyhow::Result<crate::cron_serve::TickReport>),
        Signalled(&'static str),
    }

    'run: loop {
        tokio::select! {
            _ = interval.tick() => {
                let tick = crate::cron_serve::tick_once(&store, &runner, &deliver, TICK_KIND);
                tokio::pin!(tick);
                let end = tokio::select! {
                    report = &mut tick => TickEnd::Finished(report),
                    reason = stop_signal.recv() => TickEnd::Signalled(reason.unwrap_or("stop")),
                };
                match end {
                    TickEnd::Finished(Ok(rep)) => {
                        log::info!("gateway: cron tick fired={} errors={}", rep.fired, rep.errors);
                    }
                    TickEnd::Finished(Err(e)) => log::warn!("gateway: cron tick failed: {e:#}"),
                    TickEnd::Signalled(reason) => {
                        exit_reason = reason;
                        drain(tick).await;
                        break 'run;
                    }
                }
            }
            _ = stop_signal.recv() => {
                exit_reason = "stop";
                break 'run;
            }
            else => break 'run,
        }
    }

    let _ = state::write(
        &home,
        &state::record(state::STATE_STOPPED, Some(exit_reason), started_at),
    );
    let _ = stop_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(3), socket_task).await;
    pid::remove_owned(&home, record.pid);
    log::info!("gateway: stopped ({exit_reason})");
    Ok(())
}

/// Portable stop signal: SIGTERM/SIGINT on unix, Ctrl-C on Windows.
/// Yields a static reason string so the state file says why we stopped.
#[cfg(unix)]
fn stop_signal() -> tokio::sync::mpsc::UnboundedReceiver<&'static str> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("SIGINT handler");
    tokio::spawn(async move {
        tokio::select! {
            _ = sigterm.recv() => { let _ = tx.send("SIGTERM"); }
            _ = sigint.recv() => { let _ = tx.send("SIGINT"); }
        }
    });
    rx
}

/// Portable stop signal: SIGTERM/SIGINT on unix, Ctrl-C on Windows.
/// Yields a static reason string so the state file says why we stopped.
#[cfg(not(unix))]
fn stop_signal() -> tokio::sync::mpsc::UnboundedReceiver<&'static str> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = tx.send("stop");
    });
    rx
}

/// Bounded drain for an in-flight fire (see module docs).
async fn drain<F: std::future::Future<Output = anyhow::Result<crate::cron_serve::TickReport>>>(
    tick: std::pin::Pin<&mut F>,
) {
    log::info!("gateway: draining in-flight cron tick (up to {DRAIN_SECS}s)…");
    match tokio::time::timeout(Duration::from_secs(DRAIN_SECS), tick).await {
        Ok(Ok(rep)) => log::info!(
            "gateway: drained — fired={} errors={}",
            rep.fired,
            rep.errors
        ),
        Ok(Err(e)) => log::warn!("gateway: drained with tick error: {e:#}"),
        Err(_) => log::warn!(
            "gateway: drain window elapsed; abandoning the fire (its claim expires within 300s)"
        ),
    }
}
