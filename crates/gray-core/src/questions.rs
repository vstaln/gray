//! User-question bridge: lets a tool pause the agent turn and collect
//! structured answers from the user (codex `request_user_input` port).
//! Interactive gray wires a TUI overlay; piped gray reads stdin.

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::error::CoreError;

/// One selectable option shown to the user (codex `ToolRequestUserInputOption`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UserOption {
    pub label: String,
    pub description: String,
}

/// One question in a request (codex `ToolRequestUserInputQuestion`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UserQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<UserOption>,
    /// Frontend adds an extra free-form "Other" option (codex forces this true).
    #[serde(default)]
    pub is_other: bool,
}

/// Answers for one question: selected option label plus optional notes
/// (codex `ToolRequestUserInputAnswer`: `[label…, "user_note: …"]`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UserAnswer {
    pub id: String,
    pub answers: Vec<String>,
}

/// Bridge between tools and whatever collects user input (TUI overlay in
/// interactive mode, stdin prompts in piped mode).
pub trait QuestionAsker: Send + Sync {
    /// With `blocking`, resolves with the user's answers. Without, shows the
    /// questions and returns immediately (frontend auto-resolves or delivers
    /// answers out of band — gray injects them as a follow-up user message).
    fn ask(
        &self,
        questions: Vec<UserQuestion>,
        blocking: bool,
    ) -> BoxFuture<'static, Result<Vec<UserAnswer>, CoreError>>;
}

/// Debug wrapper so `ToolContext` keeps its derived `Debug` (trait objects
/// are not `Debug`). `Clone` shares the bridge across turn contexts.
#[derive(Clone)]
pub struct QuestionBridge(pub Arc<dyn QuestionAsker>);

impl std::fmt::Debug for QuestionBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("QuestionBridge")
    }
}
