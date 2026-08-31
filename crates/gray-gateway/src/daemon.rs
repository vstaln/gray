use std::collections::HashMap;
use std::sync::Arc;
use crate::config::{GatewayConfig, Platform, load_gateway_config};
use crate::platform::BasePlatformAdapter;
use crate::session::{build_session_key, shared_store, GatewaySessionStore};
use crate::telegram::TelegramAdapter;
use crate::discord::DiscordAdapter;
use crate::slack::SlackAdapter;
pub struct GatewayRunner { pub config: GatewayConfig, pub adapters: HashMap<Platform, Box<dyn BasePlatformAdapter>>, pub store: Arc<dyn GatewaySessionStore> }
impl GatewayRunner {
    pub fn from_config(config: GatewayConfig) -> anyhow::Result<Self> {
        let store = shared_store();
        let mut adapters: HashMap<Platform, Box<dyn BasePlatformAdapter>> = HashMap::new();
        for (plat, cfg) in &config.platforms {
            if !cfg.enabled { continue; }
            let adapter: Box<dyn BasePlatformAdapter> = match plat {
                Platform::Telegram => Box::new(TelegramAdapter::new(cfg.clone())?),
                Platform::Discord => Box::new(DiscordAdapter::new(cfg.clone())?),
                Platform::Slack => Box::new(SlackAdapter::new(cfg.clone())?),
            };
            adapters.insert(*plat, adapter);
        }
        Ok(Self{config, adapters, store})
    }
    pub async fn start(&mut self) -> anyhow::Result<()> {
        for (plat, adapter) in self.adapters.iter_mut() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(45), adapter.connect()).await;
        }
        if self.adapters.is_empty() { anyhow::bail!("no gateway platforms enabled — edit ~/.gray/gateway.yaml"); }
        #[cfg(unix)] {
            let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
            tokio::select! { _ = sigterm.recv() => {}, _ = sigint.recv() => {} }
        }
        #[cfg(not(unix))] { tokio::signal::ctrl_c().await?; }
        self.stop().await
    }
    pub async fn stop(&mut self) -> anyhow::Result<()> {
        for (_, a) in self.adapters.iter_mut() { let _ = a.disconnect().await; }
        Ok(())
    }
}
pub async fn run_gateway() -> anyhow::Result<()> {
    let cfg = load_gateway_config();
    let mut runner = GatewayRunner::from_config(cfg)?;
    runner.start().await
}
