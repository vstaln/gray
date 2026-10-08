//! `gray gateway run`: the foreground daemon the supervisor executes.
//!
//! Three things run here: the control socket, the brain (events → turns →
//! outbox; see `brain.rs`), and the 60s cron ticker on
//! its own thread. Shutdown is deliberate (hermes drain doctrine): a stop
//! signal interrupts turns in flight (their admission records resume them
//! on the next start), gives an in-flight cron fire a bounded drain window,
//! closes the socket, releases the pid claim, and records why we stopped.

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
    // Post-mortem observability only: chain (never replace) the `main.rs`
    // hook, and best-effort record the panic so `gateway status` stops
    // reporting a stale `running` after a crash. Writes via the same atomic
    // state path as the normal stop record.
    let panic_home = home.clone();
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = state::write(
            &panic_home,
            &state::record(
                state::STATE_STOPPED,
                Some(&format!("panic: {info}")),
                started_at,
            ),
        );
        prev_hook(info);
    }));
    log::info!(
        "gateway: starting (pid {}, home {})",
        record.pid,
        home.display()
    );
    // The same restart record chat adapters keep: a run that never said
    // goodbye is logged as a crash, not as a quiet start.
    let lifecycle_dir = home.join("gateway");
    match crate::gateway::lifecycle::boot(&lifecycle_dir) {
        crate::gateway::lifecycle::Previous::Crashed => {
            log::warn!("gateway: previous run ended unexpectedly (crash, kill, or reboot)")
        }
        previous => log::info!("gateway: previous run: {}", previous.as_str()),
    }
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

    // Cron runs on its own thread (its agent future is not `Send`), so a
    // ten-minute job no longer freezes the event loop or the
    // socket. It drains on the same stop signal.
    let (cron_stop_tx, cron_stop_rx) = tokio::sync::watch::channel(false);
    let cron_thread = {
        let (config, home) = (config.clone(), home.clone());
        std::thread::Builder::new()
            .name("gray-cron".into())
            .spawn(move || cron_thread(config, home, cron_stop_rx))?
    };

    // The brain: events → lanes → child turns → outbox.
    let gray_bin = std::env::current_exe().unwrap_or_else(|_| "gray".into());
    let runner = std::sync::Arc::new(crate::gateway::turn::ChildRunner {
        gray_bin,
        home: home.clone(),
    });
    let mut brain = crate::gateway::brain::Brain::new(&home, runner);
    brain.recover();
    let mut done = brain.take_done();
    let mut poll = tokio::time::interval(Duration::from_secs(POLL_SECS));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut stop_signal = stop_signal();
    let exit_reason: &'static str;
    loop {
        tokio::select! {
            _ = poll.tick() => brain.step(crate::cron::now_secs()),
            Some(finished) = done.recv() => {
                brain.finish(finished);
                brain.step(crate::cron::now_secs());
            }
            reason = stop_signal.recv() => {
                exit_reason = reason.unwrap_or("stop");
                break;
            }
        }
    }
    // Turns that already finished are delivered, not re-run next boot.
    while let Ok(finished) = done.try_recv() {
        brain.finish(finished);
    }
    // Turns in flight are interrupted, not waited for (they can run for
    // half an hour); their admission records resume them on next start.
    brain.abort_all();
    let _ = cron_stop_tx.send(true);
    let _ = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(DRAIN_SECS + 5);
        while !cron_thread.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
    })
    .await;

    let restart = crate::gateway::lifecycle::mark_stopped(&lifecycle_dir);
    let exit_reason = if restart { "restart" } else { exit_reason };
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

/// Seconds between brain polls (trigger files, waiting
/// events written by other processes). Socket requests nudge it at once.
const POLL_SECS: u64 = 2;

/// The 60s cron ticker, on its own current-thread runtime. A stop request
/// lets an in-flight fire drain (bounded) before the thread ends.
fn cron_thread(
    config: Config,
    home: std::path::PathBuf,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log::warn!("gateway: cron thread has no runtime, cron disabled: {e:#}");
            return;
        }
    };
    rt.block_on(async move {
        let store = match crate::cron::CronStore::open(home.join("cron")) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("gateway: cron store unavailable, cron disabled: {e:#}");
                return;
            }
        };
        let runner = crate::cron_serve::HeadlessRunner {
            config,
            follow_switches: true,
        };
        let deliver = crate::cron_serve::SaveLocalDeliver { home: home.clone() };
        // Aligned to wall-clock :00 so recurring schedules fire on minute
        // boundaries instead of drifting.
        let first = tokio::time::Instant::now()
            + Duration::from_secs(60 - (crate::cron::now_secs() as u64 % 60));
        let mut interval = tokio::time::interval_at(first, Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let tick = crate::cron_serve::tick_once(&store, &runner, &deliver, TICK_KIND);
                    tokio::pin!(tick);
                    tokio::select! {
                        report = &mut tick => match report {
                            Ok(rep) => log::info!("gateway: cron tick fired={} errors={}", rep.fired, rep.errors),
                            Err(e) => log::warn!("gateway: cron tick failed: {e:#}"),
                        },
                        _ = stop.changed() => {
                            drain(tick).await;
                            return;
                        }
                    }
                }
                _ = stop.changed() => return,
            }
        }
    });
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
