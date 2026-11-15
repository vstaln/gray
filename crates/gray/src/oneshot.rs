use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use gray_core::agent::Provider;
use gray_core::event::StreamEvent;
use gray_core::message::ChatRequest;

const TIMEOUT: Duration = Duration::from_secs(40);

pub(crate) async fn complete(
    provider: Arc<dyn Provider>,
    req: ChatRequest,
) -> Result<String, String> {
    let mut stream = provider.stream(req);
    let drain = async {
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::TextDelta { delta }) => text.push_str(&delta),
                Ok(StreamEvent::MessageComplete { .. }) => break,
                Ok(_) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(text)
    };
    tokio::time::timeout(TIMEOUT, drain)
        .await
        .unwrap_or_else(|_| Err("completion timed out".to_string()))
}
