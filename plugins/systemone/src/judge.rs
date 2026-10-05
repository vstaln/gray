//! `judge` tool: ask typed questions about a state, get probabilities back.

use serde_json::{Value, json};

use crate::client;
use crate::config::Config;

pub const MAX_QUESTIONS: usize = 64;
const MAX_CHOICE_OPTIONS: usize = 255;
const SCORE_LEVELS: std::ops::RangeInclusive<usize> = 2..=10;

fn is_state_shape(v: &Value) -> bool {
    v.is_string() || v.is_object() || v.is_array()
}

/// Validate `args` before any network call. Returns a precise message on
/// the first problem found; Ok carries nothing (args pass through as-is).
pub fn validate(args: &Value) -> Result<(), String> {
    match args.get("state") {
        Some(s) if is_state_shape(s) => {}
        Some(_) => {
            return Err("judge: `state` must be a string, object, or array".into());
        }
        None => return Err("judge: missing required `state`".into()),
    }
    let questions = match args.get("questions") {
        Some(Value::Object(q)) => q,
        Some(_) => return Err("judge: `questions` must be an object (id → question)".into()),
        None => return Err("judge: missing required `questions`".into()),
    };
    if questions.is_empty() || questions.len() > MAX_QUESTIONS {
        return Err(format!(
            "judge: `questions` must have 1..={MAX_QUESTIONS} entries (got {})",
            questions.len()
        ));
    }
    for (id, q) in questions {
        let obj = q
            .as_object()
            .ok_or_else(|| format!("judge: question `{id}` must be an object"))?;
        let ty = obj
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("judge: question `{id}` needs a `type` (choice|score|noul)"))?;
        match obj.get("instructions") {
            Some(v) if v.is_string() || v.is_object() || v.is_array() => {}
            Some(_) => {
                return Err(format!(
                    "judge: question `{id}` `instructions` must be a string, object, or array"
                ));
            }
            None => return Err(format!("judge: question `{id}` missing `instructions`")),
        }
        match ty {
            "choice" => {
                let criteria = obj
                    .get("criteria")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        format!("judge: choice question `{id}` needs `criteria` object")
                    })?;
                if criteria.is_empty() || criteria.len() > MAX_CHOICE_OPTIONS {
                    return Err(format!(
                        "judge: choice question `{id}` needs 1..={MAX_CHOICE_OPTIONS} criteria keys"
                    ));
                }
                for (k, v) in criteria {
                    let ok = v.is_string() || v.is_object() || v.is_array() || v.is_null();
                    if !ok {
                        return Err(format!(
                            "judge: choice question `{id}` criteria `{k}` must be string, \
                             object, array, or null"
                        ));
                    }
                }
            }
            "score" => {
                let criteria = obj
                    .get("criteria")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        format!("judge: score question `{id}` needs `criteria` array")
                    })?;
                if !SCORE_LEVELS.contains(&criteria.len()) {
                    return Err(format!(
                        "judge: score question `{id}` needs {}..={} criteria entries (got {})",
                        SCORE_LEVELS.start(),
                        SCORE_LEVELS.end(),
                        criteria.len()
                    ));
                }
            }
            "noul" => {
                if let Some(c) = obj.get("criteria") {
                    let m = c.as_object().ok_or_else(|| {
                        format!("judge: noul question `{id}` `criteria` must be an object")
                    })?;
                    for k in m.keys() {
                        if k != "true" && k != "false" {
                            return Err(format!(
                                "judge: noul question `{id}` criteria only allow `true`/`false` \
                                 keys (got `{k}`)"
                            ));
                        }
                    }
                }
            }
            other => {
                return Err(format!(
                    "judge: question `{id}` has unknown type `{other}` (choice|score|noul)"
                ));
            }
        }
    }
    Ok(())
}

/// Pull the yes-probability out of one noul answer. The typed wire shape
/// is `{"type":"noul","noul":0.95}`; a bare number is accepted for
/// simpler backends.
pub fn answer_probability(v: &Value) -> Option<f64> {
    v.get("noul").and_then(Value::as_f64).or_else(|| v.as_f64())
}

/// Compact success content shared by judge and semantic_find.
pub fn result_content(cfg: &Config, resp: &Value, answers: Value) -> String {
    let model = resp
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(&cfg.model);
    serde_json::to_string(&json!({
        "backend": cfg.base,
        "model": model,
        "answers": answers,
        "usage": resp.get("usage").cloned().unwrap_or(Value::Null),
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

/// Run the tool: validate → POST → compact JSON content.
/// `Err(msg)` is the `is_error` content string.
pub fn run(cfg: &Config, args: &Value) -> Result<String, String> {
    validate(args)?;
    let body = json!({
        "state": args["state"],
        "model": cfg.model,
        "questions": args["questions"],
    });
    let resp = client::post_systemone(cfg, &body, std::time::Duration::from_secs(20))?;
    let answers = resp.get("answers").cloned().unwrap_or_else(|| resp.clone());
    Ok(result_content(cfg, &resp, answers))
}

#[path = "judge_tests.rs"]
#[cfg(test)]
mod tests;
