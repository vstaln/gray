use super::*;

#[derive(Default)]
pub(super) struct DynamicToolCall {
    pub(super) index: usize,
    pub(super) name: String,
    pub(super) args: String,
    pub(super) done: bool,
}

#[derive(Default)]
pub(super) struct EventTrail {
    pub(super) count: usize,
    pub(super) tail: VecDeque<String>,
}

impl EventTrail {
    pub(super) const TAIL_CAP: usize = 8;

    pub(super) fn push(&mut self, typ: &str) {
        self.count += 1;
        if self.tail.len() == Self::TAIL_CAP {
            self.tail.pop_front();
        }
        self.tail.push_back(typ.to_string());
    }

    pub(super) fn tail_joined(&self) -> String {
        self.tail.iter().cloned().collect::<Vec<_>>().join(",")
    }
}

pub(super) enum DynamicState {
    Init {
        client: reqwest::Client,
        url: Url,
        profile: Box<OpenAiProviderProfile>,
        lease: CredentialLease,
        body: Box<ResponsesRequest>,
        session_id: Option<String>,
        isolated: bool,
    },
    Streaming {
        event_stream: BoxedEventStream,
        tools: BTreeMap<String, DynamicToolCall>,
        next_index: usize,
        pending: VecDeque<StreamEvent>,
        last_usage: Option<Usage>,
        completed: bool,
        trail: EventTrail,
        reasoning_deltas: HashSet<String>,
    },
    Done,
}

pub(super) fn redact_lease(mut snippet: String, bearer: Option<&str>) -> String {
    if let Some(bearer) = bearer.filter(|value| !value.is_empty()) {
        snippet = snippet.replace(bearer, "[redacted]");
    }
    snippet
}

pub(super) fn dynamic_headers(
    profile: &OpenAiProviderProfile,
    lease: &CredentialLease,
    session_id: Option<&str>,
) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    for header in &profile.headers {
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|_| ProviderError::BadRequest("invalid provider header name".into()))?;
        if header.name.len() > 128 {
            return Err(ProviderError::BadRequest(
                "provider header name too long".into(),
            ));
        }
        let value = match &header.source {
            OpenAiHeaderSource::Static(value) => value.clone(),
            OpenAiHeaderSource::Metadata(key) => {
                lease.metadata.get(key).cloned().ok_or_else(|| {
                    ProviderError::BadRequest(format!("missing provider metadata header {key}"))
                })?
            }
            OpenAiHeaderSource::SessionId => session_id.unwrap_or_default().to_string(),
        };
        if value.is_empty() {
            if header.required {
                return Err(ProviderError::BadRequest(format!(
                    "missing provider header {}",
                    header.name
                )));
            }
            continue;
        }
        if value.len() > 4096 || value.contains('\r') || value.contains('\n') {
            return Err(ProviderError::BadRequest(
                "invalid provider header value".into(),
            ));
        }
        let value = HeaderValue::from_str(&value)
            .map_err(|_| ProviderError::BadRequest("invalid provider header value".into()))?;
        headers.insert(name, value);
    }
    let OpenAiAuthorization::Bearer { secret_name } = &profile.authorization;
    let bearer = lease
        .secrets
        .get(secret_name)
        .ok_or_else(|| ProviderError::Auth("provider bearer credential missing".into()))?;
    let value = HeaderValue::from_str(&format!("Bearer {bearer}"))
        .map_err(|_| ProviderError::Auth("invalid provider bearer credential".into()))?;
    headers.insert(AUTHORIZATION, value);
    Ok(headers)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn send_dynamic_json_once(
    client: &reqwest::Client,
    url: &Url,
    profile: &OpenAiProviderProfile,
    lease: &CredentialLease,
    body: &ResponsesRequest,
    session_id: Option<&str>,
    isolated: bool,
) -> Result<reqwest::Response, ProviderError> {
    let mut headers = dynamic_headers(profile, lease, session_id)?;
    mark_isolated(&mut headers, &lease.metadata, isolated);
    let OpenAiAuthorization::Bearer { secret_name } = &profile.authorization;
    let bearer = lease.secrets.get(secret_name).map(str::to_owned);
    let response = send_with_retries("dynamic send", || {
        client
            .post(url.clone())
            .headers(headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "text/event-stream")
            .json(body)
    })
    .await
    .map_err(map_send_error)?;
    let status = response.status();
    if !status.is_success() {
        let mut snippet = String::new();
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| ProviderError::Stream(e.to_string()))?
        {
            snippet.push_str(&String::from_utf8_lossy(&chunk));
            if snippet.len() > MAX_DYNAMIC_ERROR_BYTES {
                snippet.truncate(MAX_DYNAMIC_ERROR_BYTES);
                break;
            }
        }
        let snippet = redact_lease(snippet, bearer.as_deref());
        return Err(classify_http_error(status, &snippet, None, None));
    }
    log::debug!(target: "gray_provider", "dynamic Responses returned {status}");
    Ok(response)
}

/// Short human message for a transport-level send failure, with a hint for
/// the likely cause. The raw reqwest text is kept on a second line.
pub(crate) fn friendly_send_error(err: &reqwest::Error) -> String {
    let raw = err.to_string();
    let Some(url) = err.url() else {
        return raw;
    };
    let host = url.host_str().unwrap_or("");
    let hostport = url
        .port_or_known_default()
        .map_or_else(|| host.to_string(), |port| format!("{host}:{port}"));

    let mut refused = false;
    let mut dns = false;
    let mut src = err.source();
    while let Some(cause) = src {
        if cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionRefused)
        {
            refused = true;
        }
        let text = cause.to_string().to_lowercase();
        if text.contains("connection refused")
            || text.contains("os error 111")
            || text.contains("os error 61")
            || text.contains("os error 10061")
        {
            refused = true;
        }
        if text.contains("dns")
            || text.contains("failed to lookup")
            || text.contains("no such host")
            || text.contains("name or service not known")
            || text.contains("nodename nor servname")
            || text.contains("temporary failure in name resolution")
        {
            dns = true;
        }
        src = cause.source();
    }

    let friendly = if refused {
        format!("nothing is listening at {hostport} — is the server running?")
    } else if dns {
        format!("can't resolve {host} — check the address")
    } else if err.is_timeout() {
        format!("{hostport} didn't answer in time — is the server reachable?")
    } else if err.is_connect() {
        format!("can't connect to {hostport} — is the server reachable?")
    } else {
        return raw;
    };
    format!("{friendly}\n{raw}")
}

pub(super) fn map_send_error(error: reqwest::Error) -> ProviderError {
    if error.is_connect() {
        ProviderError::Connection(friendly_send_error(&error))
    } else if error.is_timeout() {
        ProviderError::Timeout(friendly_send_error(&error))
    } else {
        ProviderError::Stream(error.to_string())
    }
}

pub(super) fn reasoning_summary_text(item: &Value) -> String {
    item.get("summary")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

pub(super) fn process_dynamic_event(
    value: Value,
    pending: &mut VecDeque<StreamEvent>,
    tools: &mut BTreeMap<String, DynamicToolCall>,
    next_index: &mut usize,
    last_usage: &mut Option<Usage>,
    reasoning_deltas: &mut HashSet<String>,
) -> Result<bool, ProviderError> {
    let typ = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match typ {
        "response.output_text.delta" => {
            if let Some(delta) = value.get("delta").and_then(|v| v.as_str())
                && !delta.is_empty()
            {
                pending.push_back(StreamEvent::text_delta(delta));
            }
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            if typ.ends_with("summary_text.delta")
                && let Some(id) = value.get("item_id").and_then(|v| v.as_str())
                && !id.is_empty()
            {
                reasoning_deltas.insert(id.to_string());
            }
            if let Some(delta) = value.get("delta").and_then(|v| v.as_str())
                && !delta.is_empty()
            {
                pending.push_back(StreamEvent::thinking_delta(delta));
            }
        }
        "response.output_item.added" => {
            let item = value.get("item");
            if item.and_then(|i| i.get("type")).and_then(|t| t.as_str()) == Some("function_call") {
                let id = item
                    .and_then(|i| i.get("call_id"))
                    .and_then(|v| v.as_str())
                    .or_else(|| item.and_then(|i| i.get("id")).and_then(|v| v.as_str()))
                    .unwrap_or_default()
                    .to_string();
                let name = item
                    .and_then(|i| i.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                if !id.is_empty() {
                    let index = *next_index;
                    *next_index += 1;
                    tools.insert(
                        id.clone(),
                        DynamicToolCall {
                            index,
                            name: name.clone(),
                            ..Default::default()
                        },
                    );
                    pending.push_back(StreamEvent::tool_call_delta(
                        index,
                        Some(id),
                        Some(name),
                        "",
                    ));
                }
            }
        }
        "response.function_call_arguments.delta" => {
            let id = value
                .get("item_id")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("call_id").and_then(|v| v.as_str()))
                .unwrap_or_default();
            if let Some(delta) = value.get("delta").and_then(|v| v.as_str())
                && let Some(call) = tools.get_mut(id)
            {
                call.args.push_str(delta);
            }
        }
        "response.output_item.done" => {
            let item = value.get("item");
            let item_type = item.and_then(|i| i.get("type")).and_then(|t| t.as_str());
            if item_type == Some("reasoning") {
                let id = item
                    .and_then(|i| i.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let encrypted = item
                    .and_then(|i| i.get("encrypted_content"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let summary = item.map(reasoning_summary_text).unwrap_or_default();
                if !summary.is_empty() && !reasoning_deltas.contains(id) {
                    pending.push_back(StreamEvent::thinking_delta(summary));
                }
                if !id.is_empty() && !encrypted.is_empty() {
                    pending.push_back(StreamEvent::reasoning_item(id, encrypted));
                }
            } else if item_type == Some("function_call") {
                let id = item
                    .and_then(|i| i.get("call_id"))
                    .and_then(|v| v.as_str())
                    .or_else(|| item.and_then(|i| i.get("id")).and_then(|v| v.as_str()))
                    .unwrap_or_default()
                    .to_string();
                if let Some(arguments) = item
                    .and_then(|i| i.get("arguments"))
                    .and_then(|v| v.as_str())
                    && let Some(call) = tools.get_mut(&id)
                {
                    call.args.push_str(arguments);
                }
                if let Some(call) = tools.get_mut(&id)
                    && !call.done
                {
                    call.done = true;
                    let index = call.index;
                    let name = call.name.clone();
                    let args = normalize_tool_args(&call.args, index)?;
                    pending.push_back(StreamEvent::tool_call_delta(
                        index,
                        Some(id),
                        Some(name),
                        args,
                    ));
                }
            }
        }
        "response.completed" => {
            if let Some(uval) = value
                .get("response")
                .and_then(|r| r.get("usage"))
                .or_else(|| value.get("usage"))
                && let Ok(u) = serde_json::from_value::<OpenAiUsageChunk>(uval.clone())
            {
                *last_usage = Some(map_usage(&u));
            }
            let stop = if tools.is_empty() {
                StopReason::EndTurn
            } else {
                StopReason::ToolUse
            };
            pending.push_back(StreamEvent::message_complete(Some(stop), *last_usage));
            return Ok(true);
        }
        "response.incomplete" => {
            let reason = value
                .get("response")
                .and_then(|r| r.get("incomplete_details"))
                .and_then(|d| d.get("reason"))
                .and_then(|v| v.as_str());
            let stop = if reason == Some("max_output_tokens") {
                StopReason::MaxTokens
            } else {
                StopReason::Error
            };
            pending.push_back(StreamEvent::message_complete(Some(stop), *last_usage));
            return Ok(true);
        }
        "response.failed" | "error" => {
            let detail = value
                .get("response")
                .and_then(|r| r.get("error"))
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .or_else(|| value.get("message").and_then(|v| v.as_str()))
                .unwrap_or("provider stream failed");
            return Err(stream_failure(&value, detail.to_string()));
        }
        _ => {}
    }
    Ok(false)
}

pub(super) fn stream_failure(event: &Value, detail: String) -> ProviderError {
    let code = event
        .get("response")
        .and_then(|r| r.get("error"))
        .or_else(|| event.get("error"))
        .unwrap_or(event)
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    if quota_hinted(&[code.as_str(), detail.as_str()]) {
        let resets_in = parse_quota_reset_delay(&detail);
        return ProviderError::Detailed {
            class: Box::new(ProviderError::Quota {
                message: detail.clone(),
                resets_in,
            }),
            detail: Box::new(gray_core::error::ProviderDetail {
                status: None,
                vendor_code: Some(quota_vendor_code(
                    (!code.is_empty()).then_some(code),
                    resets_in,
                )),
                vendor_message: Some(detail),
                cf_ray: None,
                request_id: None,
                retry_after_ms: resets_in.map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64),
            }),
        };
    }
    ProviderError::Stream(detail)
}

pub(super) async fn dynamic_step(
    initial: DynamicState,
) -> Option<(Result<StreamEvent, ProviderError>, DynamicState)> {
    let mut state = initial;
    loop {
        match state {
            DynamicState::Init {
                client,
                url,
                profile,
                lease,
                body,
                session_id,
                isolated,
            } => {
                match send_dynamic_json_once(
                    &client,
                    &url,
                    &profile,
                    &lease,
                    &body,
                    session_id.as_deref(),
                    isolated,
                )
                .await
                {
                    Ok(response) => {
                        // No stall guard: plugin relays are turn-settled — silence
                        // until the turn completes is the contract, not a dead
                        // connection. The sidecar's own turn deadline bounds it.
                        let event_stream = sse_event_stream(response);
                        state = DynamicState::Streaming {
                            event_stream,
                            tools: BTreeMap::new(),
                            next_index: 0,
                            pending: VecDeque::new(),
                            last_usage: None,
                            completed: false,
                            trail: EventTrail::default(),
                            reasoning_deltas: HashSet::new(),
                        };
                    }
                    Err(error) => return Some((Err(error), DynamicState::Done)),
                }
            }
            DynamicState::Streaming {
                mut event_stream,
                mut tools,
                mut next_index,
                mut pending,
                mut last_usage,
                mut completed,
                mut trail,
                mut reasoning_deltas,
            } => {
                if completed && pending.is_empty() {
                    return None;
                }
                let next = event_stream.next().await;
                match next {
                    None => {
                        log::warn!(target: "gray_provider", "responses stream ended before response.completed: events={} tail=[{}] pending_tool_calls={}",
                            trail.count,
                            trail.tail_joined(),
                            tools.values().filter(|c| !c.done).count());
                        return Some((
                            Err(ProviderError::Stream(
                                "dynamic Responses stream ended before response.completed".into(),
                            )),
                            DynamicState::Done,
                        ));
                    }
                    Some(Err(error)) => {
                        return Some((
                            Err(ProviderError::Stream(error.to_string())),
                            DynamicState::Done,
                        ));
                    }
                    Some(Ok(event)) => {
                        let data = event.data.trim();
                        if data.is_empty() {
                            state = DynamicState::Streaming {
                                event_stream,
                                tools,
                                next_index,
                                pending,
                                last_usage,
                                completed,
                                trail,
                                reasoning_deltas,
                            };
                            continue;
                        }
                        if data == "[DONE]" {
                            log::warn!(target: "gray_provider", "responses stream ended before response.completed: events={} tail=[{}] pending_tool_calls={}",
                                trail.count,
                                trail.tail_joined(),
                                tools.values().filter(|c| !c.done).count());
                            return Some((
                                Err(ProviderError::Stream(
                                    "dynamic Responses stream ended before response.completed"
                                        .into(),
                                )),
                                DynamicState::Done,
                            ));
                        }
                        let value: Value = match serde_json::from_str(data) {
                            Ok(value) => value,
                            Err(e) => {
                                return Some((
                                    Err(ProviderError::Stream(format!(
                                        "failed to parse Responses SSE chunk: {e}: {data}"
                                    ))),
                                    DynamicState::Done,
                                ));
                            }
                        };
                        trail.push(value.get("type").and_then(|v| v.as_str()).unwrap_or(""));
                        match process_dynamic_event(
                            value,
                            &mut pending,
                            &mut tools,
                            &mut next_index,
                            &mut last_usage,
                            &mut reasoning_deltas,
                        ) {
                            Ok(finished) => completed |= finished,
                            Err(error) => return Some((Err(error), DynamicState::Done)),
                        }
                        if !pending.is_empty() {
                            let event = pending.pop_front().expect("pending event");
                            state = DynamicState::Streaming {
                                event_stream,
                                tools,
                                next_index,
                                pending,
                                last_usage,
                                completed,
                                trail,
                                reasoning_deltas,
                            };
                            return Some((Ok(event), state));
                        }
                        if completed {
                            return None;
                        }
                        state = DynamicState::Streaming {
                            event_stream,
                            tools,
                            next_index,
                            pending,
                            last_usage,
                            completed,
                            trail,
                            reasoning_deltas,
                        };
                    }
                }
            }
            DynamicState::Done => return None,
        }
    }
}

pub(super) const MAX_DYNAMIC_ERROR_BYTES: usize = 4096;

/// Marks a relay request as a side request (compaction summary) that is
/// not the next turn of the conversation.
pub const ISOLATED_HEADER: &str = "x-gray-isolated";

/// Side requests (compaction summaries) must not continue a relay's live
/// conversation: a pooled relay would otherwise take one for the next turn
/// and supersede the tool calls still parked in it. Only relays get the
/// header — real endpoints never see it.
pub(super) fn mark_isolated(
    headers: &mut reqwest::header::HeaderMap,
    metadata: &std::collections::BTreeMap<String, String>,
    isolated: bool,
) {
    if isolated && metadata.contains_key("relay_url") {
        headers.insert(
            reqwest::header::HeaderName::from_static(ISOLATED_HEADER),
            reqwest::header::HeaderValue::from_static("1"),
        );
    }
}

impl OpenAiProvider {
    pub(super) fn dynamic_stream(
        &self,
        request: ChatRequest,
        profile: OpenAiProviderProfile,
        credential_source: Arc<dyn CredentialSource>,
        session_id: Option<String>,
        cache_off: bool,
        isolated: bool,
    ) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        let mut body = map_chat_to_responses(
            request,
            &self.model,
            session_id.as_deref(),
            self.reasoning_effort.as_deref(),
        );
        apply_dynamic_policy(&mut body, &profile.request, session_id.as_deref());
        body.temperature = self.temperature;
        body.top_p = self.top_p;
        if cache_off {
            body.prompt_cache_key = None;
        }

        let profile_url = match responses_url(&profile.base_url) {
            Ok(url) => url,
            Err(error) => return stream::once(async move { Err(error) }).boxed(),
        };
        let client = self.http.clone();
        let started = stream::once(async move {
            credential_source
                .acquire()
                .await
                .map_err(|e| match e {
                    // The credential source is unreachable (sidecar died
                    // mid-request, timed out, refresh endpoint offline):
                    // transient, so the turn retries instead of telling
                    // the user to sign in again.
                    gray_core::credential::CredentialError::Unavailable(_) => {
                        ProviderError::Connection(format!("provider credential unavailable: {e:#}"))
                    }
                    _ => ProviderError::Auth(format!("provider credential unavailable: {e:#}")),
                })
                .and_then(|lease| {
                    let url = match lease.metadata.get("relay_url") {
                        Some(relay) => relay.parse::<Url>().map_err(|e| {
                            ProviderError::BadRequest(format!("invalid relay URL: {e}"))
                        }),
                        None => Ok(profile_url.clone()),
                    }?;
                    Ok((lease, body, url))
                })
        });
        started
            .flat_map(move |result| match result {
                Ok((lease, body, url)) => stream::unfold(
                    DynamicState::Init {
                        client: client.clone(),
                        url,
                        profile: Box::new(profile.clone()),
                        lease,
                        body: Box::new(body),
                        session_id: session_id.clone(),
                        isolated,
                    },
                    dynamic_step,
                )
                .boxed(),
                Err(error) => stream::once(async move { Err(error) }).boxed(),
            })
            .boxed()
    }
}

#[cfg(test)]
mod isolated_header_tests {
    use super::*;

    fn meta(relay: bool) -> std::collections::BTreeMap<String, String> {
        let mut m = std::collections::BTreeMap::new();
        if relay {
            m.insert(
                "relay_url".into(),
                "http://127.0.0.1:1/relay/x/responses".into(),
            );
        }
        m
    }

    #[test]
    fn only_isolated_relay_requests_are_marked() {
        for (relay, isolated, marked) in [
            (true, true, true),
            (true, false, false),
            (false, true, false),
            (false, false, false),
        ] {
            let mut h = reqwest::header::HeaderMap::new();
            mark_isolated(&mut h, &meta(relay), isolated);
            assert_eq!(
                h.contains_key(ISOLATED_HEADER),
                marked,
                "relay={relay} isolated={isolated}"
            );
        }
    }
}
