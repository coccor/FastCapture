//! The Claude API backend: one Messages API request per note, with the note schema as structured
//! output, the writing rules cached as the system prompt, and the API's own refusal fallback.

use super::backend::{self, Backend, BackendError};
use super::request::NoteRequest;
use crate::net::http::{Client, Request};
use crate::note::{NoteJson, reply, schema};
use serde_json::{Value, json};
use std::time::Duration;

pub const API_BASE: &str = "https://api.anthropic.com";
pub const API_VERSION: &str = "2023-06-01";
/// Lets the API rerun a safety decline on another model inside the same call.
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";

/// The models Settings offers, cheapest last.
pub const MODELS: [(&str, &str); 3] = [
    ("claude-sonnet-5-5", "Claude Sonnet 5.5"),
    ("claude-opus-5-5", "Claude Opus 5.5"),
    ("claude-haiku-5-5", "Claude Haiku 5.5"),
];

const SERVICE: &str = "Claude";
const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_TOKENS: u32 = 16_000;
/// Writing a note from a capture is a short task.
pub const DEFAULT_EFFORT: &str = "low";

pub struct Claude {
    client: Client,
    base: String,
    model: String,
    key: String,
    effort: &'static str,
    last_usage: std::sync::Mutex<Option<Usage>>,
}

/// The tokens one request used, as the API counted them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Usage {
    /// Input not read from the cache.
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl Claude {
    pub fn new(model: &str, key: String) -> Result<Self, BackendError> {
        Self::with_base(API_BASE, model, key)
    }

    /// For tests: another address than the real API.
    pub fn with_base(base: &str, model: &str, key: String) -> Result<Self, BackendError> {
        let client = Client::new(&format!("FastCapture/{}", env!("CARGO_PKG_VERSION")))
            .map_err(|error| backend::from_http(error, SERVICE))?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            key,
            effort: DEFAULT_EFFORT,
            last_usage: std::sync::Mutex::new(None),
        })
    }

    /// The tokens the last request used, when the API said.
    pub fn last_usage(&self) -> Option<Usage> {
        self.last_usage.lock().ok().and_then(|usage| *usage)
    }

    /// The same backend at another effort (`low`, `medium`, `high`…), for comparing them.
    pub fn with_effort(mut self, effort: &'static str) -> Self {
        self.effort = effort;
        self
    }

    /// The request body for `request`.
    pub fn body(&self, request: &NoteRequest) -> Value {
        let mut content: Vec<Value> = request
            .images
            .iter()
            .map(|image| {
                json!({
                    "type": "image",
                    "source": {"type": "base64", "media_type": image.media_type, "data": image.base64()}
                })
            })
            .collect();
        content.push(json!({"type": "text", "text": request.user_text}));
        json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": [{"type": "text", "text": request.system, "cache_control": {"type": "ephemeral"}}],
            "output_config": {
                "effort": self.effort,
                "format": {"type": "json_schema", "schema": schema::schema()}
            },
            "fallbacks": "default",
            "messages": [{"role": "user", "content": content}]
        })
    }
}

impl Backend for Claude {
    fn label(&self) -> String {
        format!("cloud/{}", self.model)
    }

    fn complete(&self, request: &NoteRequest) -> Result<NoteJson, BackendError> {
        let body = self.body(request).to_string();
        let response = self
            .client
            .send(&Request {
                method: "POST",
                url: &format!("{}/v1/messages", self.base),
                headers: vec![
                    ("content-type", "application/json".to_owned()),
                    ("x-api-key", self.key.clone()),
                    ("anthropic-version", API_VERSION.to_owned()),
                    ("anthropic-beta", FALLBACK_BETA.to_owned()),
                ],
                body: body.as_bytes(),
                timeout: TIMEOUT,
            })
            .map_err(|error| backend::from_http(error, SERVICE))?;
        if response.status != 200 {
            return Err(backend::from_status(&response, SERVICE));
        }
        let message: Value =
            serde_json::from_slice(&response.body).map_err(|error| BackendError::Retry {
                after: None,
                reason: format!("Claude sent a reply that isn't JSON: {error}"),
            })?;
        if let Some(usage) = message.get("usage") {
            let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
            let usage = Usage {
                input: count("input_tokens"),
                cache_read: count("cache_read_input_tokens"),
                cache_write: count("cache_creation_input_tokens"),
                output: count("output_tokens"),
            };
            crate::log::line(&format!(
                "claude: {} input ({} cached), {} output tokens",
                usage.input, usage.cache_read, usage.output
            ));
            if let Ok(mut last) = self.last_usage.lock() {
                *last = Some(usage);
            }
        }
        match message.get("stop_reason").and_then(Value::as_str) {
            Some("refusal") => {
                return Err(BackendError::Fail(
                    "Claude declined to write a note for this capture.".to_owned(),
                ));
            }
            Some("max_tokens") => {
                return Err(BackendError::Fail(
                    "Claude's note was cut off at the length limit.".to_owned(),
                ));
            }
            _ => {}
        }
        let text = message
            .get("content")
            .and_then(Value::as_array)
            .and_then(|blocks| {
                blocks.iter().find_map(|block| {
                    (block.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| block.get("text").and_then(Value::as_str))
                        .flatten()
                })
            })
            .ok_or_else(|| {
                BackendError::Invalid(reply::ReplyError(vec!["the reply has no text".to_owned()]))
            })?;
        reply::parse(text).map_err(BackendError::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::Usage;
    use super::*;
    use crate::model::images::ModelImage;
    use crate::model::request::CaptureFacts;
    use crate::net::test_server::{Scripted, TestServer};
    use std::path::PathBuf;

    fn request() -> NoteRequest {
        NoteRequest {
            system: crate::model::prompt::RULES,
            user_text: "<your_note>\nlag\n</your_note>".to_owned(),
            images: vec![ModelImage {
                media_type: "image/png",
                data: b"png".to_vec(),
                width: 1,
                height: 1,
            }],
            capture: CaptureFacts {
                notebook: PathBuf::from(r"D:\Notes"),
                capture_id: "ID".into(),
                captured: "2026-10-19T10:30".into(),
                raw: PathBuf::from(r"raw\x.md"),
                kinds: vec!["text".into()],
                attachments: Vec::new(),
                fallback_title: "lag".into(),
                links: Vec::new(),
                truncated: Vec::new(),
            },
        }
    }

    const NOTE: &str = r#"{"title":"Lag","summary":"S","body":"B","key_points":[],"tasks":[],"tags":["ops"],"links":[],"home":null}"#;

    fn message(stop: &str, text: &str) -> String {
        json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-5-5",
            "content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": text}],
            "stop_reason": stop,
            "usage": {"input_tokens": 1200, "cache_read_input_tokens": 900, "output_tokens": 210}
        })
        .to_string()
    }

    #[test]
    fn the_request_has_the_schema_the_cached_rules_and_the_image_first() {
        let server = TestServer::start(vec![Scripted::json(200, &message("end_turn", NOTE))]);
        let claude = Claude::with_base(&server.url(), DEFAULT_MODEL, "sk-ant-test".into()).unwrap();
        let note = claude.complete(&request()).unwrap();
        assert_eq!(note.title, "Lag");
        assert_eq!(note.tags, ["ops"]);
        assert_eq!(claude.label(), "cloud/claude-sonnet-5-5");
        assert_eq!(
            claude.last_usage(),
            Some(Usage {
                input: 1200,
                cache_read: 900,
                cache_write: 0,
                output: 210
            })
        );

        let seen = &server.requests()[0];
        assert_eq!(seen.path, "/v1/messages");
        assert_eq!(seen.header("x-api-key"), Some("sk-ant-test"));
        assert_eq!(seen.header("anthropic-version"), Some(API_VERSION));
        assert_eq!(seen.header("anthropic-beta"), Some(FALLBACK_BETA));
        let body = seen.json();
        assert_eq!(body["model"], "claude-sonnet-5-5");
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(body["output_config"]["format"]["schema"], schema::schema());
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["system"][0]["text"], crate::model::prompt::RULES);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["data"], "cG5n");
        assert_eq!(content[1]["text"], "<your_note>\nlag\n</your_note>");
        assert!(
            body.get("thinking").is_none(),
            "adaptive thinking by default"
        );
        assert!(
            !String::from_utf8_lossy(&seen.body).contains("sk-ant-test"),
            "the key is only a header"
        );
    }

    #[test]
    fn declines_cutoffs_and_bad_replies_are_told_apart() {
        let server = TestServer::start(vec![
            Scripted::json(200, &message("refusal", "")),
            Scripted::json(200, &message("max_tokens", "{\"title\":")),
            Scripted::json(200, &message("end_turn", "not json")),
        ]);
        let claude = Claude::with_base(&server.url(), DEFAULT_MODEL, "k".into()).unwrap();
        assert!(
            matches!(claude.complete(&request()), Err(BackendError::Fail(m)) if m.contains("declined"))
        );
        assert!(
            matches!(claude.complete(&request()), Err(BackendError::Fail(m)) if m.contains("cut off"))
        );
        assert!(matches!(
            claude.complete(&request()),
            Err(BackendError::Invalid(_))
        ));
    }

    #[test]
    fn http_errors_pause_wait_or_fail() {
        let error = |kind: &str, message: &str| {
            json!({"type": "error", "error": {"type": kind, "message": message}}).to_string()
        };
        let server = TestServer::start(vec![
            Scripted::json(401, &error("authentication_error", "invalid x-api-key")),
            Scripted::json(429, &error("rate_limit_error", "slow down"))
                .with_header("retry-after", "7"),
            Scripted::json(529, &error("overloaded_error", "Overloaded")),
            Scripted::json(404, &error("not_found_error", "model: claude-nope")),
        ]);
        let claude = Claude::with_base(&server.url(), "claude-nope", "k".into()).unwrap();
        assert!(matches!(
            claude.complete(&request()),
            Err(BackendError::Pause(_))
        ));
        assert!(matches!(
            claude.complete(&request()),
            Err(BackendError::Retry { after: Some(after), .. }) if after == Duration::from_secs(7)
        ));
        assert!(matches!(
            claude.complete(&request()),
            Err(BackendError::Retry { after: None, .. })
        ));
        assert!(
            matches!(claude.complete(&request()), Err(BackendError::Fail(m)) if m.contains("claude-nope"))
        );
    }
}
