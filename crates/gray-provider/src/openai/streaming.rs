use super::*;

#[allow(clippy::large_enum_variant)]
pub(super) enum StreamState {
    Init {
        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: OpenAiChatRequest,
        session_id: Option<String>,
        attempt: usize,
        retry_after: Option<Duration>,
    },
    ResponsesInit {
        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: ResponsesRequest,
        attempt: usize,
        retry_after: Option<Duration>,
        stream_attempt: usize,

        tools_by_call_id: BTreeMap<String, (usize, String, String)>,
        index_to_call_id: BTreeMap<usize, String>,
    },
    Streaming {
        event_stream: BoxedEventStream,
        accumulated_tools: BTreeMap<usize, AccumulatedToolCall>,
        last_finish_reason: Option<StopReason>,
        last_usage: Option<Usage>,
        pending_events: VecDeque<StreamEvent>,
        completed: bool,

        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: OpenAiChatRequest,
        attempt: usize,

        emitted: bool,
    },
    ResponsesStreaming {
        event_stream: BoxedEventStream,

        tools_by_call_id: BTreeMap<String, (usize, String, String)>,

        index_to_call_id: BTreeMap<usize, String>,
        last_usage: Option<Usage>,
        pending_events: VecDeque<StreamEvent>,
        completed: bool,

        saw_output_text: bool,

        last_text_part: Option<(u64, u64)>,

        client: reqwest::Client,
        url: Url,
        api_key: String,
        body: ResponsesRequest,
        stream_attempt: usize,
        last_response_id: Option<String>,
        trail: EventTrail,
        reasoning_deltas: HashSet<String>,
    },
    Done,
}

pub(super) fn normalize_tool_args(args: &str, index: usize) -> Result<String, ProviderError> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if serde_json::from_str::<Value>(trimmed).is_ok() {
        return Ok(trimmed.to_string());
    }
    let repaired = if trimmed.starts_with('{') && !trimmed.ends_with('}') {
        format!("{trimmed}}}")
    } else {
        trimmed.to_string()
    };
    if serde_json::from_str::<Value>(&repaired).is_ok() {
        return Ok(repaired);
    }
    log::warn!(target: "gray_provider", "tool call {index} malformed args: {}", args.chars().take(300).collect::<String>());
    Err(ProviderError::Stream(format!(
        "tool call at index {index} has malformed JSON arguments: {}",
        args.chars().take(500).collect::<String>()
    )))
}

#[derive(Default)]
pub(super) struct AccumulatedToolCall {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) args: String,
    pub(super) thought_signature: Option<String>,
}

pub(super) fn emit_tool_calls_and_completion(
    accumulated_tools: &mut BTreeMap<usize, AccumulatedToolCall>,
    stop_reason: Option<StopReason>,
    usage: Option<Usage>,
    pending_events: &mut VecDeque<StreamEvent>,
) -> Result<(), ProviderError> {
    for (index, call) in std::mem::take(accumulated_tools) {
        if call.name.is_empty() && call.args.trim().is_empty() {
            log::debug!(target: "gray_provider", "skipping ghost tool-call fragment at index {index} (no name, no args)");
            continue;
        }
        let args_fixed = normalize_tool_args(&call.args, index)?;
        pending_events.push_back(StreamEvent::ToolCallDelta {
            index,
            id: (!call.id.is_empty()).then_some(call.id),
            name: (!call.name.is_empty()).then_some(call.name),
            arguments_delta: args_fixed,
            thought_signature: call.thought_signature,
        });
    }

    pending_events.push_back(StreamEvent::MessageComplete { stop_reason, usage });
    Ok(())
}

pub(super) fn emit_responses_tool_calls_and_completion(
    tools_by_call_id: &mut BTreeMap<String, (usize, String, String)>,
    index_to_call_id: &mut BTreeMap<usize, String>,
    usage: Option<Usage>,
    pending_events: &mut VecDeque<StreamEvent>,
) -> Result<(), ProviderError> {
    let mut ordered: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
    for (call_id, (idx, name, args)) in std::mem::take(tools_by_call_id) {
        ordered.insert(idx, (call_id, name, args));
    }
    index_to_call_id.clear();
    for (idx, (call_id, name, args)) in ordered {
        if name.is_empty() && args.trim().is_empty() {
            log::debug!(target: "gray_provider", "skipping ghost Responses tool-call fragment at index {idx} (no name, no args)");
            continue;
        }
        let args_fixed = normalize_tool_args(&args, idx)?;
        pending_events.push_back(StreamEvent::ToolCallDelta {
            index: idx,
            id: if call_id.is_empty() {
                None
            } else {
                Some(call_id)
            },
            name: if name.is_empty() { None } else { Some(name) },
            arguments_delta: args_fixed,
            thought_signature: None,
        });
    }
    let stop_reason = if pending_events
        .iter()
        .any(|e| matches!(e, StreamEvent::ToolCallDelta { .. }))
    {
        Some(StopReason::ToolUse)
    } else {
        Some(StopReason::EndTurn)
    };
    pending_events.push_back(StreamEvent::MessageComplete { stop_reason, usage });
    Ok(())
}

pub(super) type ResponsesToolsByCallId = BTreeMap<String, (usize, String, String)>;

pub(super) type ResponsesIndexToCallId = BTreeMap<usize, String>;

pub(super) fn resume_body_and_tool_prefix(
    mut body: ResponsesRequest,
    tools_by_call_id: ResponsesToolsByCallId,
    index_to_call_id: ResponsesIndexToCallId,
    last_response_id: Option<String>,
) -> (
    ResponsesRequest,
    ResponsesToolsByCallId,
    ResponsesIndexToCallId,
) {
    let has_reasoning = body.reasoning.is_some()
        || body
            .include
            .as_ref()
            .is_some_and(|inc| inc.iter().any(|s| s == "reasoning.encrypted_content"));
    let continuing = last_response_id.is_some() && !has_reasoning;
    body.previous_response_id = if continuing { last_response_id } else { None };
    if continuing {
        (body, tools_by_call_id, index_to_call_id)
    } else {
        (body, BTreeMap::new(), BTreeMap::new())
    }
}

pub(super) fn stream_unfold_step(
    mut state: StreamState,
) -> futures::future::BoxFuture<'static, Option<(Result<StreamEvent, ProviderError>, StreamState)>>
{
    Box::pin(async move {
        loop {
            match state {
                StreamState::Init {
                    client,
                    url,
                    api_key,
                    body,
                    session_id,
                    attempt,
                    retry_after,
                } => {
                    if attempt > 1 {
                        tokio::time::sleep(backoff_delay(
                            INITIAL_BACKOFF,
                            attempt - 1,
                            retry_after,
                        ))
                        .await;
                    }
                    let body_value = match serialize_body(&body, "chat request") {
                        Ok(v) => v,
                        Err(err) => {
                            log::error!(target: "gray_provider", "dropping unsendable chat request: {err}");
                            return Some((Err(err), StreamState::Done));
                        }
                    };
                    match send_json_once(
                        &client,
                        &url,
                        &api_key,
                        &body_value,
                        attempt,
                        session_id.as_deref(),
                    )
                    .await
                    {
                        Ok(response) => {
                            let event_stream: BoxedEventStream = guarded_sse_event_stream(response);
                            state = StreamState::Streaming {
                                event_stream,
                                accumulated_tools: BTreeMap::new(),
                                last_finish_reason: None,
                                last_usage: None,
                                pending_events: VecDeque::new(),
                                completed: false,
                                client,
                                url,
                                api_key,
                                body,
                                attempt,
                                emitted: false,
                            };
                        }
                        Err((err, floor, http_status)) => {
                            if attempt == 1
                                && chat_has_reasoning_params(&body)
                                && should_retry_without_reasoning(
                                    http_status,
                                    &err.to_string(),
                                    true,
                                )
                            {
                                log::warn!(target: "gray_provider", "retrying without reasoning params after 400: {err}");
                                let mut stripped = body;
                                let model = stripped.model.clone();
                                strip_chat_reasoning_params(&mut stripped);
                                log::warn!(target: "gray_provider", "stripped reasoning params for model {model}");
                                state = StreamState::Init {
                                    client,
                                    url,
                                    api_key,
                                    body: stripped,
                                    session_id,
                                    attempt: attempt + 1,
                                    retry_after: floor,
                                };
                                continue;
                            }
                            if should_retry_request(&err, http_status, attempt)
                                && attempt < MAX_ATTEMPTS
                            {
                                log::warn!(target: "gray_provider", "retrying (attempt {attempt}) after error: {err}");
                                let next = StreamState::Init {
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    session_id,
                                    attempt: attempt + 1,
                                    retry_after: retry_floor(&err, floor, attempt),
                                };

                                if attempt == 1 {
                                    let notice = retry_notice_event(attempt, MAX_ATTEMPTS, &err);
                                    return Some((Ok(notice), next));
                                }
                                state = next;
                                continue;
                            }
                            let model = body.model.clone();
                            let err = maybe_annotate_reasoning_conflict(err, &model);
                            log::error!(target: "gray_provider", "stream request failed: {err}");
                            return Some((Err(err), StreamState::Done));
                        }
                    }
                }
                StreamState::ResponsesInit {
                    client,
                    url,
                    api_key,
                    body,
                    attempt,
                    retry_after,
                    stream_attempt,
                    tools_by_call_id,
                    index_to_call_id,
                } => {
                    if attempt > 1 {
                        tokio::time::sleep(backoff_delay(
                            INITIAL_BACKOFF,
                            attempt - 1,
                            retry_after,
                        ))
                        .await;
                    }
                    let body_value = match serialize_body(&body, "responses request") {
                        Ok(v) => v,
                        Err(err) => {
                            log::error!(target: "gray_provider", "dropping unsendable responses request: {err}");
                            return Some((Err(err), StreamState::Done));
                        }
                    };
                    match send_json_once(
                        &client,
                        &url,
                        &api_key,
                        &body_value,
                        attempt,
                        body.prompt_cache_key.as_deref(),
                    )
                    .await
                    {
                        Ok(response) => {
                            let event_stream: BoxedEventStream = guarded_sse_event_stream(response);

                            let last_response_id = body.previous_response_id.clone();
                            state = StreamState::ResponsesStreaming {
                                event_stream,
                                tools_by_call_id,
                                index_to_call_id,
                                last_usage: None,
                                pending_events: VecDeque::new(),
                                completed: false,
                                client,
                                url,
                                api_key,
                                body,
                                stream_attempt,
                                last_response_id,
                                saw_output_text: false,

                                last_text_part: None,
                                trail: EventTrail::default(),
                                reasoning_deltas: HashSet::new(),
                            };
                        }
                        Err((err, floor, http_status)) => {
                            if attempt == 1
                                && body.previous_response_id.is_some()
                                && should_retry_without_previous_response(
                                    http_status,
                                    &err.to_string(),
                                    true,
                                )
                            {
                                log::warn!(target: "gray_provider", "previous_response_id rejected, stripping and replaying once: {err}");
                                let mut stripped = body;
                                stripped.previous_response_id = None;
                                state = StreamState::ResponsesInit {
                                    client,
                                    url,
                                    api_key,
                                    body: stripped,
                                    attempt: attempt + 1,
                                    retry_after: floor,
                                    stream_attempt,

                                    tools_by_call_id: BTreeMap::new(),
                                    index_to_call_id: BTreeMap::new(),
                                };
                                continue;
                            }
                            if attempt == 1
                                && responses_has_reasoning(&body)
                                && should_retry_without_reasoning(
                                    http_status,
                                    &err.to_string(),
                                    true,
                                )
                            {
                                log::warn!(target: "gray_provider", "retrying responses without reasoning after 400: {err}");
                                let mut stripped = body;
                                let model = stripped.model.clone();
                                strip_responses_reasoning(&mut stripped);
                                log::warn!(target: "gray_provider", "stripped responses reasoning for model {model}");
                                state = StreamState::ResponsesInit {
                                    client,
                                    url,
                                    api_key,
                                    body: stripped,
                                    attempt: attempt + 1,
                                    retry_after: floor,
                                    stream_attempt,
                                    tools_by_call_id,
                                    index_to_call_id,
                                };
                                continue;
                            }
                            if should_retry_request(&err, http_status, attempt)
                                && attempt < MAX_ATTEMPTS
                            {
                                log::warn!(target: "gray_provider", "retrying responses (attempt {attempt}) after error: {err}");
                                let next = StreamState::ResponsesInit {
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    attempt: attempt + 1,
                                    retry_after: retry_floor(&err, floor, attempt),
                                    stream_attempt,
                                    tools_by_call_id,
                                    index_to_call_id,
                                };

                                if attempt == 1 {
                                    let notice = retry_notice_event(attempt, MAX_ATTEMPTS, &err);
                                    return Some((Ok(notice), next));
                                }
                                state = next;
                                continue;
                            }
                            let model = body.model.clone();
                            let err = maybe_annotate_reasoning_conflict(err, &model);
                            log::error!(target: "gray_provider", "responses request failed: {err}");
                            return Some((Err(err), StreamState::Done));
                        }
                    }
                }
                StreamState::Streaming {
                    mut event_stream,
                    mut accumulated_tools,
                    mut last_finish_reason,
                    mut last_usage,
                    mut pending_events,
                    mut completed,
                    client,
                    url,
                    api_key,
                    body,
                    attempt,
                    mut emitted,
                } => {
                    if let Some(event) = pending_events.pop_front() {
                        return Some((
                            Ok(event),
                            StreamState::Streaming {
                                event_stream,
                                accumulated_tools,
                                last_finish_reason,
                                last_usage,
                                pending_events,
                                completed,
                                client,
                                url,
                                api_key,
                                body,
                                attempt,
                                emitted,
                            },
                        ));
                    }

                    if completed {
                        return None;
                    }

                    let next = event_stream.next().await;
                    match next {
                        Some(Ok(sse_event)) => {
                            let data = sse_event.data.trim();
                            if data == "[DONE]" {
                                if !completed {
                                    completed = true;
                                    if let Err(err) = emit_tool_calls_and_completion(
                                        &mut accumulated_tools,
                                        last_finish_reason,
                                        last_usage,
                                        &mut pending_events,
                                    ) {
                                        return Some((Err(err), StreamState::Done));
                                    }
                                }
                            } else {
                                match serde_json::from_str::<OpenAiChunk>(data) {
                                    Ok(chunk) => {
                                        if let Some(u) = chunk.usage {
                                            last_usage = Some(map_usage(&u));
                                        }

                                        for choice in chunk.choices {
                                            match choice.delta.content {
                                                Some(delta_text) if !delta_text.is_empty() => {
                                                    pending_events.push_back(
                                                        StreamEvent::TextDelta {
                                                            delta: delta_text,
                                                        },
                                                    );
                                                    emitted = true;
                                                }
                                                _ => {}
                                            }

                                            let reasoning = choice
                                                .delta
                                                .reasoning_content
                                                .or(choice.delta.reasoning)
                                                .or(choice.delta.thought)
                                                .or(choice.delta.thoughts);
                                            if let Some(reasoning) = reasoning
                                                && !reasoning.is_empty()
                                            {
                                                pending_events.push_back(
                                                    StreamEvent::ThinkingDelta { delta: reasoning },
                                                );
                                                emitted = true;
                                            }

                                            if let Some(tool_calls) = choice.delta.tool_calls {
                                                for tc in tool_calls {
                                                    let signature = tc
                                                        .extra_content
                                                        .and_then(|extra| extra.google)
                                                        .and_then(|google| google.thought_signature)
                                                        .filter(|s| !s.is_empty());
                                                    let carries_anything = tc.id.is_some()
                                                        || signature.is_some()
                                                        || tc.function.as_ref().is_some_and(|f| {
                                                            f.name.is_some()
                                                                || f.arguments.is_some()
                                                        });
                                                    if !carries_anything {
                                                        continue;
                                                    }
                                                    let entry = accumulated_tools
                                                        .entry(tc.index)
                                                        .or_default();
                                                    if let Some(signature) = signature {
                                                        if entry
                                                            .thought_signature
                                                            .as_ref()
                                                            .is_some_and(|s| s != &signature)
                                                        {
                                                            return Some((
                                                                Err(ProviderError::Stream(
                                                                    format!(
                                                                        "provider changed thought signature for tool-call index {}",
                                                                        tc.index
                                                                    ),
                                                                )),
                                                                StreamState::Done,
                                                            ));
                                                        }
                                                        entry.thought_signature = Some(signature);
                                                    }
                                                    if let Some(id) = tc.id {
                                                        entry.id.push_str(&id);
                                                    }
                                                    if let Some(func) = tc.function {
                                                        if let Some(name) = func.name {
                                                            entry.name.push_str(&name);
                                                        }
                                                        if let Some(args) = func.arguments {
                                                            entry.args.push_str(&args);
                                                        }
                                                    }
                                                }
                                            }

                                            if let Some(reason_str) = choice.finish_reason {
                                                last_finish_reason = map_finish_reason(&reason_str);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        return Some((
                                            Err(ProviderError::Stream(format!(
                                                "failed to parse OpenAI SSE chunk: {e}"
                                            ))),
                                            StreamState::Done,
                                        ));
                                    }
                                }
                            }

                            state = StreamState::Streaming {
                                event_stream,
                                accumulated_tools,
                                last_finish_reason,
                                last_usage,
                                pending_events,
                                completed,
                                client,
                                url,
                                api_key,
                                body,
                                attempt,
                                emitted,
                            };
                        }
                        Some(Err(err)) => {
                            log::error!(target: "gray_provider", "stream error: {err}");

                            if !emitted && attempt < MAX_ATTEMPTS && is_midstream_transport(&err) {
                                log::warn!(
                                    target: "gray_provider",
                                    "stream body failed before any delta; retrying (attempt {attempt}): {err}"
                                );
                                let next = StreamState::Init {
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    session_id: None,
                                    attempt: attempt + 1,
                                    retry_after: None,
                                };

                                if attempt == 1 {
                                    let notice = retry_notice_event(
                                        attempt,
                                        MAX_ATTEMPTS,
                                        &ProviderError::Stream(err.to_string()),
                                    );
                                    return Some((Ok(notice), next));
                                }
                                state = next;
                                continue;
                            }
                            if emitted && accumulated_tools.is_empty() {
                                let details = err.to_string().chars().take(200).collect::<String>();
                                pending_events.push_back(StreamEvent::stream_error(
                                    "Connection interrupted; partial answer kept",
                                    format!("{details}; send another message to retry"),
                                ));
                                pending_events.push_back(StreamEvent::MessageComplete {
                                    stop_reason: Some(StopReason::EndTurn),
                                    usage: last_usage,
                                });
                                completed = true;
                                state = StreamState::Streaming {
                                    event_stream,
                                    accumulated_tools,
                                    last_finish_reason,
                                    last_usage,
                                    pending_events,
                                    completed,
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    attempt,
                                    emitted,
                                };
                                continue;
                            }
                            return Some((
                                Err(ProviderError::Stream(err.to_string())),
                                StreamState::Done,
                            ));
                        }
                        None => {
                            if last_finish_reason.is_none() {
                                let complete = accumulated_tools.values().all(|call| {
                                    let trimmed = call.args.trim();
                                    !trimmed.is_empty()
                                        && serde_json::from_str::<Value>(trimmed).is_ok()
                                });
                                if completed || accumulated_tools.is_empty() || !complete {
                                    let err = ProviderError::Stream(
                                        "Chat stream ended without a finish reason".into(),
                                    );

                                    if !emitted && !completed && attempt < MAX_ATTEMPTS {
                                        log::warn!(
                                            target: "gray_provider",
                                            "stream ended without a finish reason before any delta; retrying (attempt {attempt})"
                                        );
                                        let next = StreamState::Init {
                                            client,
                                            url,
                                            api_key,
                                            body,
                                            session_id: None,
                                            attempt: attempt + 1,
                                            retry_after: None,
                                        };

                                        if attempt == 1 {
                                            let notice =
                                                retry_notice_event(attempt, MAX_ATTEMPTS, &err);
                                            return Some((Ok(notice), next));
                                        }
                                        state = next;
                                        continue;
                                    }
                                    return Some((Err(err), StreamState::Done));
                                }
                                match emit_tool_calls_and_completion(
                                    &mut accumulated_tools,
                                    Some(StopReason::ToolUse),
                                    last_usage,
                                    &mut pending_events,
                                ) {
                                    Ok(()) => {
                                        log::warn!(
                                            target: "gray_provider",
                                            "stream ended without a finish reason; keeping complete tool calls"
                                        );
                                        completed = true;
                                        last_finish_reason = Some(StopReason::ToolUse);
                                        state = StreamState::Streaming {
                                            event_stream,
                                            accumulated_tools,
                                            last_finish_reason,
                                            last_usage,
                                            pending_events,
                                            completed,
                                            client,
                                            url,
                                            api_key,
                                            body,
                                            attempt,
                                            emitted: true,
                                        };
                                    }
                                    Err(_) => {
                                        return Some((
                                            Err(ProviderError::Stream(
                                                "Chat stream ended without a finish reason".into(),
                                            )),
                                            StreamState::Done,
                                        ));
                                    }
                                }
                            } else if !completed {
                                completed = true;
                                if let Err(err) = emit_tool_calls_and_completion(
                                    &mut accumulated_tools,
                                    last_finish_reason,
                                    last_usage,
                                    &mut pending_events,
                                ) {
                                    return Some((Err(err), StreamState::Done));
                                }
                                state = StreamState::Streaming {
                                    event_stream,
                                    accumulated_tools,
                                    last_finish_reason,
                                    last_usage,
                                    pending_events,
                                    completed,
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    attempt,
                                    emitted: true,
                                };
                            } else {
                                return None;
                            }
                        }
                    }
                }
                StreamState::ResponsesStreaming {
                    mut event_stream,
                    mut tools_by_call_id,
                    mut index_to_call_id,
                    mut last_usage,
                    mut pending_events,
                    mut completed,
                    mut saw_output_text,
                    mut last_text_part,
                    client,
                    url,
                    api_key,
                    body,
                    stream_attempt,
                    mut last_response_id,
                    mut trail,
                    mut reasoning_deltas,
                } => {
                    if let Some(event) = pending_events.pop_front() {
                        return Some((
                            Ok(event),
                            StreamState::ResponsesStreaming {
                                event_stream,
                                tools_by_call_id,
                                index_to_call_id,
                                last_usage,
                                pending_events,
                                completed,
                                client,
                                url,
                                api_key,
                                body,
                                stream_attempt,
                                last_response_id,
                                saw_output_text,
                                last_text_part,
                                trail,
                                reasoning_deltas,
                            },
                        ));
                    }
                    if completed {
                        return None;
                    }
                    let next = event_stream.next().await;
                    match next {
                        Some(Ok(sse_event)) => {
                            let data = sse_event.data.trim();
                            if data.is_empty() {
                                state = StreamState::ResponsesStreaming {
                                    event_stream,
                                    tools_by_call_id,
                                    index_to_call_id,
                                    last_usage,
                                    pending_events,
                                    completed,
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    stream_attempt,
                                    last_response_id,
                                    saw_output_text,
                                    last_text_part,
                                    trail,
                                    reasoning_deltas,
                                };
                                continue;
                            }
                            let value: Value = match serde_json::from_str(data) {
                                Ok(v) => v,
                                Err(e) => {
                                    return Some((
                                        Err(ProviderError::Stream(format!(
                                            "failed to parse Responses SSE chunk: {e}: {data}"
                                        ))),
                                        StreamState::Done,
                                    ));
                                }
                            };
                            let typ = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            trail.push(typ);

                            if let Some(rid) = value
                                .get("response_id")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .or_else(|| {
                                    value
                                        .get("response")
                                        .and_then(|r| r.get("id"))
                                        .and_then(|v| v.as_str())
                                        .filter(|s| !s.is_empty())
                                })
                            {
                                last_response_id = Some(rid.to_string());
                            }
                            match typ {
                                "response.output_text.delta" => {
                                    if let Some(delta) = value.get("delta").and_then(|v| v.as_str())
                                        && !delta.is_empty()
                                    {
                                        saw_output_text = true;

                                        let part = (
                                            value
                                                .get("output_index")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0),
                                            value
                                                .get("content_index")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0),
                                        );
                                        let mut delta = delta.to_string();
                                        if last_text_part.is_some_and(|p| p != part) {
                                            delta.insert_str(0, "\n\n");
                                        }
                                        last_text_part = Some(part);
                                        pending_events.push_back(StreamEvent::TextDelta { delta });
                                    }
                                }
                                "response.reasoning_text.delta"
                                | "response.reasoning_summary_text.delta" => {
                                    if typ.ends_with("summary_text.delta")
                                        && let Some(id) =
                                            value.get("item_id").and_then(|v| v.as_str())
                                        && !id.is_empty()
                                    {
                                        reasoning_deltas.insert(id.to_string());
                                    }
                                    if let Some(delta) = value.get("delta").and_then(|v| v.as_str())
                                        && !delta.is_empty()
                                    {
                                        pending_events.push_back(StreamEvent::ThinkingDelta {
                                            delta: delta.to_string(),
                                        });
                                    }
                                }
                                "response.output_item.added" => {
                                    if let Some(item) = value.get("item") {
                                        let item_type =
                                            item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                        if item_type == "function_call" {
                                            let call_id = item
                                                .get("call_id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let item_id = item
                                                .get("id")
                                                .and_then(|v| v.as_str())
                                                .map(|s| s.to_string())
                                                .unwrap_or_else(|| call_id.clone());
                                            let name = item
                                                .get("name")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            if !call_id.is_empty()
                                                && !tools_by_call_id.contains_key(&call_id)
                                            {
                                                let idx = tools_by_call_id.len();
                                                tools_by_call_id.insert(
                                                    call_id.clone(),
                                                    (idx, name.clone(), String::new()),
                                                );
                                                index_to_call_id.insert(idx, call_id.clone());
                                                if item_id != call_id && !item_id.is_empty() {
                                                    tools_by_call_id.insert(
                                                        item_id.clone(),
                                                        (idx, name, String::new()),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                                "response.function_call_arguments.delta" => {
                                    let item_id =
                                        value.get("item_id").and_then(|v| v.as_str()).unwrap_or("");
                                    let delta =
                                        value.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                                    if !delta.is_empty() && !item_id.is_empty() {
                                        if let Some(entry) = tools_by_call_id.get_mut(item_id) {
                                            entry.2.push_str(delta);
                                        } else {
                                            let idx = tools_by_call_id.len();
                                            tools_by_call_id.insert(
                                                item_id.to_string(),
                                                (idx, String::new(), delta.to_string()),
                                            );
                                            index_to_call_id.insert(idx, item_id.to_string());
                                        }
                                    }
                                }
                                "response.function_call_arguments.done" => {
                                    if let Some(args) =
                                        value.get("arguments").and_then(|v| v.as_str())
                                    {
                                        let item_id = value
                                            .get("item_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        if !item_id.is_empty()
                                            && let Some(entry) = tools_by_call_id.get_mut(item_id)
                                            && entry.2.is_empty()
                                        {
                                            entry.2 = args.to_string();
                                        }
                                        if let Some(name) =
                                            value.get("name").and_then(|v| v.as_str())
                                            && let Some(entry) = tools_by_call_id.get_mut(item_id)
                                            && entry.1.is_empty()
                                        {
                                            entry.1 = name.to_string();
                                        }
                                    }
                                }
                                "response.completed" => {
                                    if !completed {
                                        completed = true;
                                        let usage_val = value
                                            .get("response")
                                            .and_then(|r| r.get("usage"))
                                            .or_else(|| value.get("usage"));
                                        if let Some(uval) = usage_val {
                                            if let Ok(u) = serde_json::from_value::<OpenAiUsageChunk>(
                                                uval.clone(),
                                            ) {
                                                last_usage = Some(map_usage(&u));
                                            } else {
                                                let input = uval
                                                    .get("input_tokens")
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0)
                                                    as usize;
                                                let output = uval
                                                    .get("output_tokens")
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0)
                                                    as usize;
                                                let cached = uval
                                                    .get("input_tokens_details")
                                                    .and_then(|d| d.get("cached_tokens"))
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0)
                                                    as usize;
                                                let reasoning = uval
                                                    .get("output_tokens_details")
                                                    .and_then(|d| d.get("reasoning_tokens"))
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0)
                                                    as usize;
                                                let total = uval
                                                    .get("total_tokens")
                                                    .and_then(|v| v.as_u64())
                                                    .map(|v| v as usize)
                                                    .unwrap_or(input + output);
                                                let mut usage = Usage {
                                                    input_tokens: input,
                                                    output_tokens: output,
                                                    reasoning_tokens: reasoning,
                                                    cached_tokens: cached,
                                                    non_cached_input_tokens: input
                                                        .saturating_sub(cached),
                                                    cache_read_input_tokens: cached,
                                                    cache_write_input_tokens: 0,
                                                    total_tokens: total,
                                                };
                                                usage.normalize();
                                                last_usage = Some(usage);
                                            }
                                        }
                                        let mut dedup: BTreeMap<usize, (String, String, String)> =
                                            BTreeMap::new();
                                        for (k, (idx, name, args)) in
                                            std::mem::take(&mut tools_by_call_id)
                                        {
                                            let entry = dedup.entry(idx).or_insert((
                                                k.clone(),
                                                String::new(),
                                                String::new(),
                                            ));
                                            if !name.is_empty() {
                                                entry.1 = name;
                                            }
                                            if !args.is_empty() {
                                                entry.2 = args.clone();
                                            }
                                            if entry.0.is_empty() || k.starts_with("call_") {
                                                entry.0 = k;
                                            }
                                        }
                                        for (idx, (call_id, name, args)) in dedup {
                                            tools_by_call_id
                                                .insert(call_id.clone(), (idx, name, args));
                                        }
                                        index_to_call_id.clear();
                                        for (call_id, (idx, _, _)) in &tools_by_call_id {
                                            index_to_call_id.insert(*idx, call_id.clone());
                                        }
                                        if let Err(err) = emit_responses_tool_calls_and_completion(
                                            &mut tools_by_call_id,
                                            &mut index_to_call_id,
                                            last_usage,
                                            &mut pending_events,
                                        ) {
                                            return Some((Err(err), StreamState::Done));
                                        }
                                    }
                                }
                                "response.incomplete" => {
                                    let reason = value["response"]["incomplete_details"]["reason"]
                                        .as_str()
                                        .unwrap_or("unknown");
                                    if reason == "max_output_tokens" {
                                        if let Some(usage) = value["response"]
                                            .get("usage")
                                            .or_else(|| value.get("usage"))
                                            && let Ok(usage) =
                                                serde_json::from_value::<OpenAiUsageChunk>(
                                                    usage.clone(),
                                                )
                                        {
                                            last_usage = Some(map_usage(&usage));
                                        }
                                        pending_events.push_back(StreamEvent::MessageComplete {
                                            stop_reason: Some(StopReason::MaxTokens),
                                            usage: last_usage,
                                        });
                                        tools_by_call_id.clear();
                                        index_to_call_id.clear();
                                        completed = true;
                                    } else {
                                        return Some((
                                            Err(ProviderError::Stream(format!(
                                                "Responses incomplete: {reason}"
                                            ))),
                                            StreamState::Done,
                                        ));
                                    }
                                }
                                "response.failed" | "error" => {
                                    let detail = value
                                        .get("response")
                                        .and_then(|r| r.get("error"))
                                        .or_else(|| value.get("error"))
                                        .or(Some(&value))
                                        .map(|e| {
                                            let code = e
                                                .get("code")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("");
                                            let msg = e
                                                .get("message")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("");
                                            format!("{code} {msg}")
                                        })
                                        .unwrap_or_else(|| typ.to_string());
                                    return Some((
                                        Err(stream_failure(
                                            &value,
                                            format!("responses stream failed: {detail}"),
                                        )),
                                        StreamState::Done,
                                    ));
                                }
                                "response.output_item.done" => {
                                    if value
                                        .get("item")
                                        .and_then(|i| i.get("type"))
                                        .and_then(|t| t.as_str())
                                        == Some("reasoning")
                                    {
                                        let item = &value["item"];
                                        let id = item
                                            .get("id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default();
                                        let ec = item
                                            .get("encrypted_content")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default();
                                        let summary = reasoning_summary_text(item);
                                        if !summary.is_empty() && !reasoning_deltas.contains(id) {
                                            pending_events
                                                .push_back(StreamEvent::thinking_delta(summary));
                                        }
                                        if !id.is_empty() && !ec.is_empty() {
                                            pending_events
                                                .push_back(StreamEvent::reasoning_item(id, ec));
                                        }
                                    }
                                }
                                "response.output_text.done" => {
                                    saw_output_text = true;
                                }
                                "ping"
                                | "response.created"
                                | "response.in_progress"
                                | "response.content_part.added"
                                | "response.content_part.done"
                                | "response.reasoning_text.done" => {}
                                _ => {
                                    if let Some(uval) = value
                                        .get("response")
                                        .and_then(|r| r.get("usage"))
                                        .or_else(|| value.get("usage"))
                                        && let Ok(u) =
                                            serde_json::from_value::<OpenAiUsageChunk>(uval.clone())
                                    {
                                        last_usage = Some(map_usage(&u));
                                    }
                                }
                            }
                            state = StreamState::ResponsesStreaming {
                                event_stream,
                                tools_by_call_id,
                                index_to_call_id,
                                last_usage,
                                pending_events,
                                completed,
                                client,
                                url,
                                api_key,
                                body,
                                stream_attempt,
                                last_response_id,
                                saw_output_text,
                                last_text_part,
                                trail,
                                reasoning_deltas,
                            };
                        }
                        Some(Err(err)) => {
                            if stream_attempt < MAX_ATTEMPTS
                                && !matches!(err, SseReadError::Stalled(_))
                            {
                                log::warn!(target: "gray_provider", "responses stream error, resuming (stream attempt {stream_attempt}): {err}");

                                tokio::time::sleep(backoff_delay(
                                    INITIAL_BACKOFF,
                                    stream_attempt,
                                    None,
                                ))
                                .await;
                                let (resumed, resume_tools, resume_index) =
                                    resume_body_and_tool_prefix(
                                        body,
                                        tools_by_call_id,
                                        index_to_call_id,
                                        last_response_id,
                                    );
                                state = StreamState::ResponsesInit {
                                    client,
                                    url,
                                    api_key,
                                    body: resumed,
                                    attempt: 1,
                                    retry_after: None,
                                    stream_attempt: stream_attempt + 1,
                                    tools_by_call_id: resume_tools,
                                    index_to_call_id: resume_index,
                                };
                                continue;
                            }
                            log::error!(target: "gray_provider", "responses stream error: {err}");
                            return Some((
                                Err(ProviderError::Stream(err.to_string())),
                                StreamState::Done,
                            ));
                        }
                        None => {
                            if tools_by_call_id.is_empty() && saw_output_text {
                                log::warn!(
                                    target: "gray_provider",
                                    "responses stream ended without response.completed; completing text output"
                                );
                                completed = true;
                                pending_events.push_back(StreamEvent::MessageComplete {
                                    stop_reason: Some(StopReason::EndTurn),
                                    usage: last_usage,
                                });
                                state = StreamState::ResponsesStreaming {
                                    event_stream,
                                    tools_by_call_id,
                                    index_to_call_id,
                                    last_usage,
                                    pending_events,
                                    completed,
                                    saw_output_text,
                                    last_text_part,
                                    client,
                                    url,
                                    api_key,
                                    body,
                                    stream_attempt,
                                    last_response_id,
                                    trail,
                                    reasoning_deltas,
                                };
                                continue;
                            }

                            let pending_tool_calls = tools_by_call_id
                                .values()
                                .map(|(idx, _, _)| *idx)
                                .collect::<std::collections::BTreeSet<_>>()
                                .len();
                            log::warn!(target: "gray_provider", "responses stream ended before response.completed: events={} tail=[{}] pending_tool_calls={} saw_output_text={} response_id={}",
                                trail.count,
                                trail.tail_joined(),
                                pending_tool_calls,
                                saw_output_text,
                                if last_response_id.is_some() { "some" } else { "none" });
                            return Some((
                                Err(ProviderError::Stream(
                                    "Responses stream ended before response.completed".into(),
                                )),
                                StreamState::Done,
                            ));
                        }
                    }
                }
                StreamState::Done => {
                    return None;
                }
            }
        }
    })
}
