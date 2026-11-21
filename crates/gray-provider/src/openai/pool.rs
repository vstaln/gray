use super::*;
use gray_core::credential::{CredentialError, CredentialFailure};

/// Drives one request across an API-key pool: acquire the serving key, run
/// the request, and on a failure that blames the key report it and restart
/// on the next one. Output already streamed is never replayed, so a
/// rotation only happens before the first delta.
pub(super) struct PoolDrive {
    pub(super) provider: OpenAiProvider,
    pub(super) request: ChatRequest,
    pub(super) caps: ModelCaps,
    pub(super) session_id: Option<String>,
    pub(super) cache_off: bool,
    pub(super) source: Arc<dyn CredentialSource>,
    pub(super) inner: Option<BoxStream<'static, Result<StreamEvent, ProviderError>>>,
    pub(super) lease: Option<CredentialLease>,
    pub(super) tried: Vec<String>,
    pub(super) failed: Option<ProviderError>,
    pub(super) emitted: bool,
    pub(super) done: bool,
}

/// The pool verdict for a failed request, from the classification the
/// provider already made. `None` means the failure says nothing about the
/// key, so nothing is parked.
pub(crate) fn credential_failure(
    err: &ProviderError,
    retry_after: Option<Duration>,
) -> Option<CredentialFailure> {
    match err.class() {
        ProviderError::RateLimited(_) => Some(CredentialFailure::RateLimited { retry_after }),
        ProviderError::Quota { resets_in, .. } => Some(CredentialFailure::Quota {
            resets_in: *resets_in,
        }),
        ProviderError::Auth(_) => Some(CredentialFailure::Unauthorized),
        _ => None,
    }
}

fn unavailable(e: CredentialError) -> ProviderError {
    match e {
        CredentialError::Unavailable(msg) => ProviderError::Connection(msg),
        CredentialError::Cooling { message, resets_in } => ProviderError::Quota {
            message,
            resets_in: Some(resets_in),
        },
        other => ProviderError::Auth(format!("provider credential unavailable: {other}")),
    }
}

/// Hand the driver back with the failure, marked finished so the next poll
/// ends the stream.
fn finish(
    mut drive: PoolDrive,
    err: ProviderError,
) -> Option<(Result<StreamEvent, ProviderError>, PoolDrive)> {
    drive.done = true;
    Some((Err(err), drive))
}

fn carries_output(event: &StreamEvent) -> bool {
    matches!(
        event,
        StreamEvent::TextDelta { .. }
            | StreamEvent::ThinkingDelta { .. }
            | StreamEvent::ToolCallDelta { .. }
            | StreamEvent::MessageComplete { .. }
    )
}

pub(super) fn pool_step(
    mut drive: PoolDrive,
) -> futures::future::BoxFuture<'static, Option<(Result<StreamEvent, ProviderError>, PoolDrive)>> {
    Box::pin(async move {
        loop {
            if drive.done {
                return None;
            }
            if drive.inner.is_none() {
                let lease = match drive.source.acquire().await {
                    Ok(lease) => lease,
                    Err(e) => {
                        let err = match unavailable(e) {
                            cooling @ ProviderError::Quota {
                                resets_in: Some(_), ..
                            } => cooling,
                            other => drive.failed.take().unwrap_or(other),
                        };
                        return finish(drive, err);
                    }
                };
                let Some(key) = lease
                    .secrets
                    .get(gray_core::credential::API_KEY_SECRET)
                    .map(str::to_string)
                else {
                    return finish(
                        drive,
                        ProviderError::Auth("provider credential has no API key".into()),
                    );
                };
                if drive.tried.contains(&key) {
                    let err = drive.failed.take().unwrap_or_else(|| {
                        ProviderError::Auth(
                            "every pooled API key already failed this request".into(),
                        )
                    });
                    return finish(drive, err);
                }
                log::debug!(
                    target: "gray_provider",
                    "serving request from pooled key {}",
                    lease
                        .metadata
                        .get(gray_core::credential::ACCOUNT_METADATA)
                        .map(String::as_str)
                        .unwrap_or("default")
                );
                drive.inner = Some(drive.provider.chat_stream_with_key(
                    drive.request.clone(),
                    drive.caps,
                    drive.session_id.clone(),
                    drive.cache_off,
                    &key,
                ));
                drive.lease = Some(lease);
            }
            let Some(inner) = drive.inner.as_mut() else {
                continue;
            };
            match inner.next().await {
                None => return None,
                Some(Ok(event)) => {
                    drive.emitted |= carries_output(&event);
                    return Some((Ok(event), drive));
                }
                Some(Err(err)) => {
                    drive.inner = None;
                    let failure =
                        credential_failure(&err, err.detail().and_then(|d| d.retry_after()));
                    let (Some(failure), Some(lease), false) =
                        (failure, drive.lease.clone(), drive.emitted)
                    else {
                        return finish(drive, err);
                    };
                    drive.source.report(&lease, failure).await;
                    let Some(key) = lease
                        .secrets
                        .get(gray_core::credential::API_KEY_SECRET)
                        .map(str::to_string)
                    else {
                        return finish(drive, err);
                    };
                    if !drive.tried.contains(&key) {
                        drive.tried.push(key);
                    }
                    let details: String = err.to_string().chars().take(200).collect();
                    drive.failed = Some(err);
                    return Some((
                        Ok(StreamEvent::stream_error(
                            "Retrying on another API key".to_string(),
                            details,
                        )),
                        drive,
                    ));
                }
            }
        }
    })
}
