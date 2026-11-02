//! A chat engine served over an OpenAI-compatible HTTP API (OpenRouter by
//! default). It is a drop-in [`ChatEngine`]: prompts, actions and approval
//! work the same as with an on-device model, so switching to a local model
//! later only swaps the engine.
//!
//! The API key is passed in by the caller and never written to the data file.

use std::time::Duration;

use serde_json::{json, Value};

use super::{ChatEngine, EngineError, EngineMessage, EngineRole};

/// OpenRouter's chat completions endpoint.
pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
/// Model used until the user picks another.
pub const DEFAULT_MODEL: &str = "openai/gpt-4o-mini";
/// Replies are short action lists, so a small cap keeps cost and latency low.
const MAX_REPLY_TOKENS: u32 = 900;
const TIMEOUT: Duration = Duration::from_secs(60);

/// An engine that sends the prompt to a hosted model.
pub struct RemoteEngine {
    url: String,
    api_key: String,
    model: String,
}

impl RemoteEngine {
    pub fn new(url: impl Into<String>, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let model = model.into();
        Self {
            url: url.into(),
            api_key: api_key.into(),
            model: if model.trim().is_empty() { DEFAULT_MODEL.to_string() } else { model.trim().to_string() },
        }
    }

    /// An engine for OpenRouter with the given key and model.
    pub fn openrouter(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new(OPENROUTER_URL, api_key, model)
    }

    /// The request body for `messages`.
    pub fn request_body(&self, messages: &[EngineMessage]) -> Value {
        let messages: Vec<Value> = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    EngineRole::System => "system",
                    EngineRole::User => "user",
                    EngineRole::Assistant => "assistant",
                };
                json!({ "role": role, "content": m.content })
            })
            .collect();
        json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0.2,
            "max_tokens": MAX_REPLY_TOKENS,
        })
    }
}

/// Reads the reply text out of a chat completions response.
pub fn reply_text(response: &Value) -> Result<String, EngineError> {
    if let Some(message) = response.pointer("/error/message").and_then(Value::as_str) {
        return Err(EngineError::Failed(message.to_string()));
    }
    response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| EngineError::Failed("the reply was empty".into()))
}

/// Plain wording for an HTTP failure.
fn http_error(status: u16, body: &str) -> EngineError {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string));
    let why = match status {
        401 => "the API key was not accepted".to_string(),
        402 => "the account has no credit left".to_string(),
        429 => "too many requests; try again in a moment".to_string(),
        _ => detail.unwrap_or_else(|| format!("the service answered with error {status}")),
    };
    EngineError::Failed(why)
}

impl ChatEngine for RemoteEngine {
    fn generate(
        &self,
        messages: &[EngineMessage],
        on_token: &mut dyn FnMut(&str) -> bool,
    ) -> Result<String, EngineError> {
        if self.api_key.trim().is_empty() {
            return Err(EngineError::NotChosen);
        }
        let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
        let response = agent
            .post(&self.url)
            .set("Authorization", &format!("Bearer {}", self.api_key.trim()))
            .set("Content-Type", "application/json")
            .set("X-Title", "Thulli")
            .send_json(self.request_body(messages));
        let body: Value = match response {
            Ok(r) => r.into_json().map_err(|e| EngineError::Failed(format!("the reply could not be read: {e}")))?,
            Err(ureq::Error::Status(code, r)) => {
                let text = r.into_string().unwrap_or_default();
                return Err(http_error(code, &text));
            }
            Err(e) => return Err(EngineError::Failed(format!("could not reach the service: {e}"))),
        };
        let text = reply_text(&body)?;
        if !on_token(&text) {
            return Err(EngineError::Cancelled);
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_carries_roles_and_model() {
        let e = RemoteEngine::openrouter("k", "");
        let body = e.request_body(&[
            EngineMessage::new(EngineRole::System, "rules"),
            EngineMessage::new(EngineRole::User, "hi"),
        ]);
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hi");
    }

    #[test]
    fn reply_text_reads_content_or_error() {
        let ok = json!({"choices":[{"message":{"content":"{\"actions\":[]}"}}]});
        assert_eq!(reply_text(&ok).unwrap(), "{\"actions\":[]}");
        let bad = json!({"error":{"message":"No auth"}});
        assert_eq!(reply_text(&bad), Err(EngineError::Failed("No auth".into())));
        assert!(reply_text(&json!({})).is_err());
    }

    #[test]
    fn missing_key_is_reported_before_any_request() {
        let e = RemoteEngine::openrouter("  ", "x");
        assert_eq!(e.generate(&[], &mut |_| true), Err(EngineError::NotChosen));
        assert_eq!(http_error(401, "").user_message(), "The assistant could not answer: the API key was not accepted");
    }
}
