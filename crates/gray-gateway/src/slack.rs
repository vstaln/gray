use crate::config::PlatformConfig;
use crate::platform::{BasePlatformAdapter, SendResult};
use crate::config::Platform;
pub struct SlackAdapter { bot_token: String }
impl SlackAdapter { pub fn new(cfg: PlatformConfig) -> anyhow::Result<Self> { Ok(Self{bot_token:cfg.token.ok_or_else(|| anyhow::anyhow!("slack token not set"))?}) } }
#[async_trait::async_trait] impl BasePlatformAdapter for SlackAdapter {
    fn platform(&self) -> Platform { Platform::Slack }
    async fn connect(&mut self) -> anyhow::Result<()> { Ok(()) }
    async fn disconnect(&mut self) -> anyhow::Result<()> { Ok(()) }
    async fn send(&self,_:&str,_:&str)->SendResult{SendResult{success:true,message_id:None,error:None,retryable:false}}
}
