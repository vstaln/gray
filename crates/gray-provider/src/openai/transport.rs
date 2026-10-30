use super::*;

pub(super) fn session_affinity_headers(session_id: Option<&str>) -> Vec<(&'static str, &str)> {
    let Some(sid) = session_id.filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    vec![("x-opencode-session", sid)]
}

pub(super) fn send_stage_retryable(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_request()
}

#[allow(clippy::result_large_err)]
pub(super) async fn send_json_once(
    client: &reqwest::Client,
    url: &Url,
    api_key: &str,
    body: &Value,
    attempt: usize,
    session_id: Option<&str>,
) -> Result<reqwest::Response, (ProviderError, Option<Duration>, u16)> {
    let mut res_result = None;
    for send_attempt in 1..=MAX_SEND_ATTEMPTS {
        let base = if api_key.is_empty() {
            client.post(url.clone())
        } else {
            client
                .post(url.clone())
                .header("Authorization", format!("Bearer {api_key}"))
        };
        let base = session_affinity_headers(if url.as_str().contains("opencode.ai/zen") {
            session_id
        } else {
            None
        })
        .into_iter()
        .fold(base, |b, (name, value)| b.header(name, value));
        let req = base.header("Content-Type", "application/json").json(body);
        match req.send().await {
            Err(err) if send_attempt < MAX_SEND_ATTEMPTS && send_stage_retryable(&err) => {
                log::warn!(target: "gray_provider", "send failed before a response ({err}); retrying on a fresh connection {send_attempt}/{MAX_SEND_ATTEMPTS}");
            }
            result => {
                res_result = Some(result);
                break;
            }
        }
    }
    let res_result = res_result.expect("send loop always yields a result");
    log::debug!(target: "gray_provider", "request sent to {url} (attempt {attempt})");
    match res_result {
        Ok(res) => {
            let status = res.status();
            if status.is_success() {
                return Ok(res);
            }
            let retry_after = parse_retry_after(res.headers());
            let cf_ray = res
                .headers()
                .get("cf-ray")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let req_id = res
                .headers()
                .get("x-request-id")
                .or_else(|| res.headers().get("request-id"))
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let text = res.text().await.unwrap_or_default();

            let snippet: String = text.chars().take(4000).collect();
            Err((
                classify_http_error(status, &snippet, cf_ray.as_deref(), req_id.as_deref()),
                retry_after,
                status.as_u16(),
            ))
        }
        Err(e) => Err((
            if e.is_connect() {
                ProviderError::Connection(friendly_send_error(&e))
            } else if e.is_timeout() {
                ProviderError::Timeout(friendly_send_error(&e))
            } else {
                ProviderError::Stream(e.to_string())
            },
            None,
            0,
        )),
    }
}

pub(super) type BoxedEventStream =
    BoxStream<'static, Result<eventsource_stream::Event, SseReadError>>;

#[derive(Debug)]
pub(super) enum SseReadError {
    Upstream(eventsource_stream::EventStreamError<reqwest::Error>),
    Stalled(Duration),
}

impl std::fmt::Display for SseReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Upstream(e) => write!(f, "{e}"),
            Self::Stalled(cap) => {
                write!(f, "stream stalled: no output for {}s", cap.as_secs())
            }
        }
    }
}

pub(super) struct StallGuard {
    pub(super) inner: BoxedEventStream,
    pub(super) cap: Duration,
    pub(super) timer: Pin<Box<tokio::time::Sleep>>,
    pub(super) fired: bool,
}

impl Stream for StallGuard {
    type Item = Result<eventsource_stream::Event, SseReadError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.fired {
            return self.inner.as_mut().poll_next(cx);
        }
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(item) => {
                if !matches!(&item, Some(Ok(ev)) if ev.event == "ping" || ev.data.trim().is_empty())
                {
                    let deadline = tokio::time::Instant::now() + self.cap;
                    self.timer.as_mut().reset(deadline);
                }
                Poll::Ready(item)
            }
            Poll::Pending => {
                if self.timer.as_mut().poll(cx).is_ready() {
                    self.fired = true;
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    LAST_STREAM_STALL_SECS.store(now, std::sync::atomic::Ordering::Relaxed);
                    return Poll::Ready(Some(Err(SseReadError::Stalled(self.cap))));
                }
                Poll::Pending
            }
        }
    }
}

pub(super) fn env_secs(name: &str) -> Option<Duration> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .map(Duration::from_secs)
}

pub(super) fn stream_stall_cap() -> Duration {
    let base = env_secs("GRAY_PROVIDER_STALL_SECS").unwrap_or(DEFAULT_STREAM_STALL);
    let last = LAST_STREAM_STALL_SECS.load(std::sync::atomic::Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if last > 0 && now.saturating_sub(last) < STREAM_STALL_MEMORY_SECS {
        base.min(env_secs("GRAY_PROVIDER_RETRY_STALL_SECS").unwrap_or(RETRY_STREAM_STALL))
    } else {
        base
    }
}

pub(super) fn stall_guard(inner: BoxedEventStream, cap: Duration) -> BoxedEventStream {
    StallGuard {
        inner,
        cap,
        timer: Box::pin(tokio::time::sleep(cap)),
        fired: false,
    }
    .boxed()
}

pub(super) fn sse_event_stream(response: reqwest::Response) -> BoxedEventStream {
    response
        .bytes_stream()
        .eventsource()
        .map(|item| item.map_err(SseReadError::Upstream))
        .boxed()
}

pub(super) fn guarded_sse_event_stream(response: reqwest::Response) -> BoxedEventStream {
    stall_guard(sse_event_stream(response), stream_stall_cap())
}
