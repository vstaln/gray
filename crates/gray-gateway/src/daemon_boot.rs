//! Gateway boot and CLI entry points (move-only split from `daemon.rs`).
//!
//! [`run_gateway`] runs until SIGINT/SIGTERM; [`run_gateway_shutdown`] and
//! [`run_gateway_shutdown_with_board`] also exit on an explicit shutdown
//! signal (REPL `/gateway stop`). [`GatewayRunner::send_startup_notifications`]
//! pings the `/restart` requester and announces the online notice.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::config::{Platform, load_gateway_config};
use crate::daemon::{GatewayRunner, take_restart_marker_in};
use crate::daemon_supervise::{BOOT_MAX_ATTEMPTS, connect_adapter_with_retry};
use crate::delivery::{DeliveryRouter, DeliveryTarget};
use crate::platform::MessageEvent;
use crate::status::GatewayStatusBoard;

impl GatewayRunner {
    /// Boot sequence: ping the `/restart`
    /// requester, then DM each platform's `home_channel`. Sends are timeout-
    /// bounded so a flood-control sleep can't freeze boot.
    pub async fn send_startup_notifications(&self) {
        if let Ok(home) = crate::config::gray_home_dir()
            && let Some(m) = take_restart_marker_in(&home)
        {
            match m.platform.parse::<Platform>() {
                Ok(p) if self.adapters.contains_key(&p) => {
                    let target = DeliveryTarget {
                        platform: p,
                        chat_id: Some(m.chat_id.clone()),
                        thread_id: None,
                        is_origin: false,
                    };
                    let r = self
                        .router
                        .deliver(
                            &target,
                            "♻ Gateway restarted successfully. Your session continues.",
                            None,
                        )
                        .await;
                    if r.success {
                        log::info!("gateway restart ping sent to {p}:{}", m.chat_id);
                    } else {
                        log::warn!("gateway restart ping failed: {:?}", r.error);
                    }
                }
                _ => log::warn!(
                    "gateway restart marker: no live adapter for '{}'",
                    m.platform
                ),
            }
        }
        // Settle beat (1s helps fresh reconnect deliveries).
        tokio::time::sleep(Duration::from_secs(1)).await;
        for (plat, r) in self.router.deliver_home_all("● Gray gateway online.").await {
            if r.success {
                log::info!("gateway online notice sent to {plat}");
            } else {
                log::warn!("gateway online notice failed for {plat}: {:?}", r.error);
            }
        }
    }
}

/// CLI entry: run until SIGINT/SIGTERM.
pub async fn run_gateway() -> anyhow::Result<()> {
    let token = tokio_util::sync::CancellationToken::new();
    let res = run_gateway_inner(token.clone(), None).await;
    token.cancel();
    res
}

/// Like [`run_gateway`], but also exits when `shutdown` resolves (REPL `/gateway stop`).
pub async fn run_gateway_shutdown(
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    run_gateway_shutdown_with_board(shutdown, None).await
}

/// Like [`run_gateway_shutdown`], but reports per-platform connect progress
/// on `board` for the REPL's live boot card (`connecting…` → `connected as …`).
pub async fn run_gateway_shutdown_with_board(
    shutdown: tokio::sync::oneshot::Receiver<()>,
    board: Option<GatewayStatusBoard>,
) -> anyhow::Result<()> {
    let token = tokio_util::sync::CancellationToken::new();
    let t = token.clone();
    let relay = tokio::spawn(async move {
        let _ = shutdown.await;
        t.cancel();
    });
    let res = run_gateway_inner(token.clone(), board).await;
    token.cancel();
    let _ = relay.await;
    res
}

async fn run_gateway_inner(
    token: tokio_util::sync::CancellationToken,
    board: Option<GatewayStatusBoard>,
) -> anyhow::Result<()> {
    let cfg = load_gateway_config();
    let mut runner = GatewayRunner::from_config(cfg)?;
    if runner.adapters.is_empty() {
        anyhow::bail!("no gateway platforms enabled — edit ~/.gray/gateway.yaml");
    }
    // Warn loudly when a platform has no operator allowlist: everyone will pair.
    for (plat, pc) in &runner.config.platforms {
        if pc.enabled
            && pc.allowed_users.is_empty()
            && std::env::var(plat.allowed_users_env()).is_err()
        {
            log::warn!(
                "gateway {plat}: no allowed_users / {} set — unknown DMs get a pairing code, groups are ignored (dm_policy={:?})",
                plat.allowed_users_env(),
                pc.dm_policy
            );
        }
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<MessageEvent>();
    // The router holds clones of the adapter Arcs; drop it so get_mut works, then rebuild.
    // Preserve dead-target tracking across the rebuild.
    let dead = Arc::clone(&runner.dead);
    runner.router =
        DeliveryRouter::new(runner.config.clone(), HashMap::new()).with_dead_targets(dead);
    for adapter in runner.adapters.values_mut() {
        match Arc::get_mut(adapter) {
            Some(a) => a.set_event_tx(tx.clone()),
            None => log::warn!("gateway: could not wire event channel (adapter shared)"),
        }
    }
    runner.rebuild_router();
    // Boot uses the lower cap so one wedged platform can't stall startup;
    // steady-state reconnects (shard/heartbeat) use MAX_RECONNECT_ATTEMPTS.
    for (plat, adapter) in runner.adapters.iter() {
        connect_adapter_with_retry(
            adapter,
            *plat,
            board.as_ref(),
            &runner.router,
            &runner.ledger,
            BOOT_MAX_ATTEMPTS,
        )
        .await;
    }
    // Boot replay: crash-recovered obligations go out before online notices.
    // (Per-adapter reconnects already swept on success above.)
    runner.sweep_pending().await;

    runner.send_startup_notifications().await;

    let runner = Arc::new(runner);
    // Agent futures are !Send (gray-core run_streaming sink), so handle events on a
    // dedicated LocalSet thread; spawn_local per event keeps /stop responsive mid-run.
    // The thread exits when `token` cancels, dropping adapters (closing connections).
    let _worker = {
        let token = token.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("gateway runtime");
            rt.block_on(tokio::task::LocalSet::new().run_until(async move {
                // Cron: run due jobs here and deliver to home channels.
                // Gated: the default `gray` build disables the `cron`
                // feature (lean tree); Task 3 re-drives cron via plugins.
                #[cfg(feature = "cron")]
                if runner.config.cron_delivery {
                    let r = Arc::clone(&runner);
                    tokio::task::spawn_local(async move {
                        let scheduler = gray_cron::Scheduler::from_active();
                        let mut interval = tokio::time::interval(Duration::from_secs(60));
                        loop {
                            interval.tick().await;
                            let due = match scheduler.scan_due_jobs() {
                                Ok(d) => d,
                                Err(e) => {
                                    log::warn!("gateway cron scan failed: {e}");
                                    continue;
                                }
                            };
                            // Sequential inline dispatch — no dedup guard needed
                            // (a claim would always release before the next scan).
                            for job in due {
                                let _ = gray_cron::store::update_job_run(&job.id, chrono::Utc::now());
                                r.run_cron_job(&job).await;
                            }
                        }
                    });
                }
                loop {
                    tokio::select! {
                        ev = rx.recv() => match ev {
                            Some(ev) => {
                                let r = Arc::clone(&runner);
                                tokio::task::spawn_local(async move {
                                    if let Err(e) = r.handle_inbound(ev).await { log::warn!("gateway handle error: {e}"); }
                                });
                            }
                            None => break,
                        },
                        _ = token.cancelled() => break,
                    }
                }
            }));
        })
    };

    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        tokio::select! {
            _ = token.cancelled() => {},
            _ = sigterm.recv() => {},
            _ = sigint.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = token.cancelled() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
    }
    Ok(())
}
