use reqwest::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAiWire {
    Responses,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenAiAuthorization {
    Bearer { secret_name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenAiHeaderSource {
    Static(String),
    Metadata(String),
    SessionId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiHeader {
    pub name: String,
    pub source: OpenAiHeaderSource,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiRequestPolicy {
    pub prompt_cache_key: bool,
    pub store: bool,
    pub include_reasoning_encrypted: bool,
    pub previous_response_id: bool,
    pub tool_choice: Option<String>,
    pub parallel_tool_calls: Option<bool>,
    pub text_verbosity: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiProviderProfile {
    pub base_url: Url,
    pub wire: OpenAiWire,
    pub authorization: OpenAiAuthorization,
    pub headers: Vec<OpenAiHeader>,
    pub request: OpenAiRequestPolicy,
    pub follow_redirects: bool,
}
