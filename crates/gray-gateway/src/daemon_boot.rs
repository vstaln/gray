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
    // Cross-process singleton: two gateway processes share one gateway.yaml /
    // one Discord token, and Discord allows concurrent sessions — without
    // this both connect and both reply to every message. `_lock` is held
    // until return; flock releases on crash, so no stale state.
    let Some(_lock) = crate::lock::try_acquire_gateway_lock() else {
        anyhow::bail!(crate::lock::ALREADY_RUNNING_MESSAGE);
    };
    let home =
        crate::config::gray_home_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp/.gray"));
    if let Some(prev) = gray_supervise::lifecycle::Lifecycle::read(&home) {
        if !prev.clean_shutdown {
            log::warn!("gateway previous exit unclean (boot {})", prev.boot_id);
        }
    }
    let _ = gray_supervise::lifecycle::Lifecycle::mark_boot(&home);
    let _ = gray_supervise::heartbeat::write_heartbeat(&home);
    let beat_home = home.clone();
    let beat_every = gray_supervise::heartbeat_interval_secs();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(beat_every)).await;
            let _ = gray_supervise::heartbeat::write_heartbeat(&beat_home);
        }
    });
    let boot_deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(gray_supervise::watchdog::startup_timeout_secs());
    let cfg = load_gateway_config();
    let mut runner = GatewayRunner::from_config(cfg)?;
    if runner.adapters.is_empty() {
        anyhow::bail!("{}", crate::config::no_platforms_message());
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
    if std::time::Instant::now() > boot_deadline {
        log::error!("gateway startup watchdog: boot exceeded budget; exiting 75");
        std::process::exit(gray_supervise::exit::EXIT_RESTART);
    }

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
            _ = sigterm.recv() => {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(gray_supervise::watchdog::SHUTDOWN_DRAIN_SECS),
                    token.cancelled(),
                )
                .await;
            },
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
    if let Ok(home) = crate::config::gray_home_dir() {
        let _ = gray_supervise::lifecycle::Lifecycle::mark_clean(&home);
    }
    Ok(())
}
