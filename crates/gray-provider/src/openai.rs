use std::collections::{BTreeMap, HashSet, VecDeque};
use std::error::Error as _;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::{self, BoxStream, Stream, StreamExt};
use gray_core::agent::{Provider, ProviderError, current_isolated_stream};
use gray_core::credential::{CredentialLease, CredentialSource};
use gray_core::event::{StopReason, StreamEvent, Usage};
use gray_core::message::{
    ChatRequest, ContentBlock, IMAGE_OMITTED, Role, TOOL_IMAGE_OMITTED, resolve_media,
};
use reqwest::Url;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use crate::openai_profile::{
    OpenAiAuthorization, OpenAiHeaderSource, OpenAiProviderProfile, OpenAiRequestPolicy, OpenAiWire,
};

pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

const MAX_ATTEMPTS: usize = 5;

const MAX_SEND_ATTEMPTS: usize = 3;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

const MAX_BACKOFF: Duration = Duration::from_secs(30);

const DEFAULT_STREAM_STALL: Duration = Duration::from_secs(300);

const RETRY_STREAM_STALL: Duration = Duration::from_secs(75);

const STREAM_STALL_MEMORY_SECS: i64 = 15 * 60;

static LAST_STREAM_STALL_SECS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

#[derive(Clone)]
pub struct OpenAiProvider {
    base_url: Url,
    api_key: String,
    model: String,
    http: reqwest::Client,
    reasoning_effort: Option<String>,

    temperature: Option<f32>,
    top_p: Option<f32>,

    session_id: Option<String>,

    profile: Option<OpenAiProviderProfile>,

    caps: ModelCaps,

    credential_source: Option<Arc<dyn CredentialSource>>,
}

impl std::fmt::Debug for OpenAiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl OpenAiProvider {
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
        reasoning_effort: Option<String>,
        session_id: Option<String>,
    ) -> Result<Self, String> {
        let base_url_str = base_url.into();
        let base_url_str = if base_url_str.is_empty() {
            DEFAULT_BASE_URL.to_string()
        } else {
            base_url_str
        };
        let base_url = Url::parse(&base_url_str)
            .map_err(|e| format!("invalid base_url '{base_url_str}': {e}"))?;

        let http = Self::http_client(true);
        let model = model.into();

        Ok(Self {
            caps: ModelCaps::for_host(&base_url),
            base_url,
            api_key: api_key.into(),
            model,
            http,
            reasoning_effort,
            temperature: None,
            top_p: None,
            session_id,
            profile: None,
            credential_source: None,
        })
    }

    fn http_client(follow_redirects: bool) -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(120))
            .redirect(if follow_redirects {
                Policy::default()
            } else {
                Policy::none()
            })
            .build()
            .expect("reqwest client with timeouts")
    }

    pub fn new_with_profile(
        model: impl Into<String>,
        reasoning_effort: Option<String>,
        session_id: Option<String>,
        profile: OpenAiProviderProfile,
        credential_source: Arc<dyn CredentialSource>,
    ) -> Result<Self, String> {
        let base_url = profile.base_url.clone();
        let http = Self::http_client(profile.follow_redirects);
        let model = model.into();
        Ok(Self {
            caps: ModelCaps::for_host(&base_url),
            base_url,
            api_key: String::new(),
            model,
            http,
            reasoning_effort,
            temperature: None,
            top_p: None,
            session_id,
            profile: Some(profile),
            credential_source: Some(credential_source),
        })
    }

    pub fn with_sampling(mut self, temperature: Option<f32>, top_p: Option<f32>) -> Self {
        self.temperature = temperature;
        self.top_p = top_p;
        self
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn model_id(&self) -> &str {
        &self.model
    }

    fn warm_output_cap(&self, _req: &ChatRequest) -> u32 {
        1
    }

    fn stream(&self, req: ChatRequest) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        let iso = current_isolated_stream().unwrap_or_default();
        let session_id = iso.session_id.or_else(|| self.session_id.clone());
        let cache_off = iso.cache_off;
        if let (Some(profile), Some(credential_source)) =
            (self.profile.clone(), self.credential_source.clone())
        {
            if !matches!(profile.wire, OpenAiWire::Responses | OpenAiWire::Auto) {
                return stream::once(async move {
                    Err(ProviderError::BadRequest(
                        "dynamic OpenAI chat completions profiles are not implemented".into(),
                    ))
                })
                .boxed();
            }
            return self.dynamic_stream(req, profile, credential_source, session_id, cache_off);
        }
        if self.caps.prefer_responses {
            let url = match responses_url(&self.base_url) {
                Ok(u) => u,
                Err(e) => return stream::once(async move { Err(e) }).boxed(),
            };
            let mut body = map_chat_to_responses(
                req,
                &self.model,
                session_id.as_deref(),
                self.reasoning_effort.as_deref(),
            );
            if cache_off {
                body.prompt_cache_key = None;
            }
            log::debug!(target: "gray_provider", "using Responses API for model {}", self.model);
            let init_state = StreamState::ResponsesInit {
                client: self.http.clone(),
                url,
                api_key: self.api_key.clone(),
                body,
                attempt: 1,
                retry_after: None,
                stream_attempt: 1,
                tools_by_call_id: BTreeMap::new(),
                index_to_call_id: BTreeMap::new(),
            };
            return stream::unfold(init_state, stream_unfold_step).boxed();
        }
        let url = match chat_completions_url(&self.base_url) {
            Ok(u) => u,
            Err(e) => return stream::once(async move { Err(e) }).boxed(),
        };

        let mut body = match map_chat_request(req, &self.model, self.reasoning_effort.as_deref()) {
            Ok(b) => b,
            Err(e) => return stream::once(async move { Err(e) }).boxed(),
        };

        body.prompt_cache_key = if cache_off {
            None
        } else {
            session_id.clone().filter(|s| !s.is_empty())
        };
        body.temperature = self.temperature;
        body.top_p = self.top_p;
        if self.caps.cap_field == CapField::MaxCompletionTokens {
            body.max_completion_tokens = body.max_tokens.take();
        }
        let init_state = StreamState::Init {
            client: self.http.clone(),
            url,
            api_key: self.api_key.clone(),
            body,
            session_id: session_id.clone(),
            attempt: 1,
            retry_after: None,
        };

        stream::unfold(init_state, stream_unfold_step).boxed()
    }
}

mod wire;
pub(crate) use wire::*;
mod request;
pub(crate) use request::*;
mod errors;
pub use errors::*;
mod transport;
use transport::*;
mod dynamic;
pub(crate) use dynamic::*;
mod streaming;
use streaming::*;

#[path = "openai_signature_tests.rs"]
#[cfg(test)]
mod signature_tests;
