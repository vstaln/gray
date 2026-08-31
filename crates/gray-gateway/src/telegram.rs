use crate::config::PlatformConfig;
use crate::platform::{BasePlatformAdapter, SendResult};
use crate::config::Platform;
pub struct TelegramAdapter { token: String }
impl TelegramAdapter { pub fn new(cfg: PlatformConfig) -> anyhow::Result<Self> { Ok(Self{token:cfg.token.ok_or_else(|| anyhow::anyhow!("telegram token not set"))?}) } }
#[async_trait::async_trait] impl BasePlatformAdapter for TelegramAdapter {
    fn platform(&self) -> Platform { Platform::Telegram }
    async fn connect(&mut self) -> anyhow::Result<()> { Ok(()) }
    async fn disconnect(&mut self) -> anyhow::Result<()> { Ok(()) }
    async fn send(&self,_:&str,_:&str)->SendResult{SendResult{success:true,message_id:None,error:None,retryable:false}}
}
