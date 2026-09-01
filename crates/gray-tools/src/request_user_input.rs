//! `request_user_input` — ask the user 1-3 multiple-choice questions and
//! wait for structured answers. 1:1 port of codex's tool spec
//! (`core/src/tools/handlers/request_user_input_spec.rs`); execution blocks
//! on the [`QuestionAsker`] bridge in `ToolContext`.

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::error::CoreError;
use gray_core::message::ToolDef;
use gray_core::questions::{UserAnswer, UserQuestion};
use serde::Deserialize;
use serde_json::{json, Value};

pub const REQUEST_USER_INPUT_TOOL_NAME: &str = "request_user_input";

#[derive(Debug, Deserialize)]
struct RequestUserInputToolArgs {
    questions: Vec<UserQuestion>,
    #[serde(default)]
    blocking: Option<bool>,
}

/// Codex normalization: options are mandatory and an "Other" option is
/// always added client-side.
pub(crate) fn normalize_request_user_input_tool_args(
    mut args: Vec<UserQuestion>,
) -> Result<Vec<UserQuestion>, String> {
    if args.iter().any(|q| q.options.is_empty()) {
        return Err("request_user_input requires non-empty options for every question".to_string());
    }
    for q in &mut args {
        q.is_other = true;
    }
    Ok(args)
}

fn answers_to_json(answers: &[UserAnswer]) -> Value {
    let map: serde_json::Map<String, Value> = answers
        .iter()
        .map(|a| (a.id.clone(), json!({ "answers": a.answers })))
        .collect();
    json!({ "answers": map })
}

pub const OTHER_OPTION_LABEL: &str = "None of the above";
pub const OTHER_OPTION_DESCRIPTION: &str = "Optionally, add details in notes (tab).";

/// Piped-mode bridge: hermes `clarify_callback` port — prints each question,
/// reads one stdin line; number picks an option, free text becomes a note,
/// EOF/blank → "use your best judgement". Non-blocking resolves immediately
/// with no answers (mirrors codex auto-resolution for unattended clients).
pub struct StdinQuestionAsker;

impl gray_core::questions::QuestionAsker for StdinQuestionAsker {
    fn ask(
        &self,
        questions: Vec<UserQuestion>,
        blocking: bool,
    ) -> futures::future::BoxFuture<'static, Result<Vec<UserAnswer>, gray_core::error::CoreError>> {
        Box::pin(async move {
            if !blocking {
                return Ok(Vec::new());
            }
            tokio::task::spawn_blocking(move || {
                use std::io::BufRead;
                let stdin = std::io::stdin();
                let total = questions.len();
                let mut out = Vec::new();
                for (idx, q) in questions.iter().enumerate() {
                    println!("\x1b[2m\x1b[1mQuestion {}/{total}: {}\x1b[0m", idx + 1, q.question);
                    for (i, opt) in q.options.iter().enumerate() {
                        println!("\x1b[2m  {}. {} — {}\x1b[0m", i + 1, opt.label, opt.description);
                    }
                    println!("\x1b[2m  {}. {OTHER_OPTION_LABEL} — {OTHER_OPTION_DESCRIPTION}\x1b[0m", q.options.len() + 1);
                    print!("\x1b[2m  answer (number, or free text; blank = skip): \x1b[0m");
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                    let mut line = String::new();
                    if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                        out.push(UserAnswer {
                            id: q.id.clone(),
                            answers: vec!["no answer — use your best judgement".to_string()],
                        });
                        continue;
                    }
                    let t = line.trim();
                    let answers = match t.parse::<usize>() {
                        Ok(n) if n >= 1 && n <= q.options.len() => {
                            vec![q.options[n - 1].label.clone()]
                        }
                        Ok(n) if n == q.options.len() + 1 => vec![OTHER_OPTION_LABEL.to_string()],
                        _ if t.is_empty() => Vec::new(),
                        _ => vec![format!("user_note: {t}")],
                    };
                    out.push(UserAnswer { id: q.id.clone(), answers });
                }
                Ok(out)
            })
            .await
            .unwrap_or_else(|e| Err(gray_core::error::CoreError::Provider(format!("stdin asker failed: {e}"))))
        })
    }
}

pub struct RequestUserInputTool;

#[async_trait]
impl super::Tool for RequestUserInputTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            REQUEST_USER_INPUT_TOOL_NAME,
            "Request user input for one to three short questions and wait for the response.",
            json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "description": "Questions to show the user. Prefer 1 and do not exceed 3",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string", "description": "Stable identifier for mapping answers (snake_case)."},
                                "header": {"type": "string", "description": "Short header label shown in the UI (12 or fewer chars)."},
                                "question": {"type": "string", "description": "Single-sentence prompt shown to the user."},
                                "options": {
                                    "type": "array",
                                    "description": "Provide 2-3 mutually exclusive choices. Put the recommended option first and suffix its label with \"(Recommended)\". Do not include an \"Other\" option in this list; the client will add a free-form \"Other\" option automatically.",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {"type": "string", "description": "User-facing label (1-5 words)."},
                                            "description": {"type": "string", "description": "One short sentence explaining impact/tradeoff if selected."}
                                        },
                                        "required": ["label", "description"],
                                        "additionalProperties": false
                                    }
                                }
                            },
                            "required": ["id", "header", "question", "options"],
                            "additionalProperties": false
                        }
                    },
                    "blocking": {
                        "type": "boolean",
                        "description": "Wait for answers before continuing (default true). When false the agent keeps working; answers arrive as a follow-up message or auto-resolve after 2 minutes."
                    }
                },
                "required": ["questions"],
                "additionalProperties": false
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&'static str> {
        Some("request_user_input — ask the user 1-3 multiple-choice questions when a decision blocks progress")
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "Use request_user_input when a concrete decision blocks progress and the choices are few; act on sensible defaults otherwise.",
            "Ask 1 question when possible (max 3); give each 2-3 mutually exclusive options, recommended first with a \"(Recommended)\" suffix.",
        ])
    }

    fn is_concurrency_safe(&self, _args: &Value) -> bool {
        false // blocks on a human
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let parsed: RequestUserInputToolArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return ToolOutput::error(format!("request_user_input: bad args: {e}")),
        };
        if parsed.questions.is_empty() || parsed.questions.len() > 3 {
            return ToolOutput::error("request_user_input requires 1-3 questions".to_string());
        }
        let questions = match normalize_request_user_input_tool_args(parsed.questions) {
            Ok(q) => q,
            Err(e) => return ToolOutput::error(e),
        };
        let blocking = parsed.blocking.unwrap_or(true);
        let Some(bridge) = &ctx.questions else {
            return ToolOutput::error(
                "request_user_input requires an interactive session (no user reachable)".to_string(),
            );
        };
        match bridge.0.ask(questions, blocking).await {
            Ok(answers) => ToolOutput::ok(answers_to_json(&answers).to_string()),
            Err(CoreError::Cancelled) => ToolOutput::error("request_user_input cancelled".to_string()),
            Err(e) => ToolOutput::error(format!("request_user_input failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::questions::UserOption;

    fn q(options: usize) -> UserQuestion {
        UserQuestion {
            id: "x".into(),
            header: "H".into(),
            question: "q?".into(),
            options: (0..options)
                .map(|i| UserOption { label: format!("o{i}"), description: "d".into() })
                .collect(),
            is_other: false,
        }
    }

    #[test]
    fn normalize_requires_options() {
        assert!(normalize_request_user_input_tool_args(vec![q(0), q(2)]).is_err());
        assert!(normalize_request_user_input_tool_args(vec![q(2), q(3)]).is_ok());
    }

    #[test]
    fn normalize_forces_is_other() {
        let out = normalize_request_user_input_tool_args(vec![q(2)]).unwrap();
        assert!(out[0].is_other);
    }

    #[test]
    fn answers_json_shape() {
        let out = answers_to_json(&[UserAnswer { id: "mode".into(), answers: vec!["fast".into(), "user_note: hurry".into()] }]);
        assert_eq!(out["answers"]["mode"]["answers"][0], "fast");
        assert_eq!(out["answers"]["mode"]["answers"][1], "user_note: hurry");
    }
}
