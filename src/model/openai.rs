//! The OpenAI-compatible backend: `POST /v1/chat/completions`, as Ollama, LM Studio and Foundry
//! Local serve it on this PC (and cloud endpoints, later). The note schema goes as
//! `response_format`; a server that rejects that gets the schema in the system prompt instead.

use super::backend::{self, Backend, BackendError};
use super::request::NoteRequest;
use crate::net::http::{Client, Request};
use crate::note::{NoteJson, reply, schema};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub struct OpenAi {
    client: Client,
    /// `http://localhost:11434`, without a trailing `/` or `/v1`.
    endpoint: String,
    model: String,
    key: Option<String>,
    /// `local` or `cloud`, for the note's `model:` line.
    mode: &'static str,
    timeout: Duration,
    /// The server rejected `response_format` once: ask with the schema in the prompt.
    schema_in_prompt: AtomicBool,
}

/// `endpoint` without a trailing `/` or `/v1`.
pub fn normalize(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    endpoint.strip_suffix("/v1").unwrap_or(endpoint).to_owned()
}

impl OpenAi {
    /// A local server: 5 minutes per request, no key.
    pub fn local(endpoint: &str, model: &str) -> Result<Self, BackendError> {
        Self::new(endpoint, model, None, "local", Duration::from_secs(300))
    }

    pub fn new(
        endpoint: &str,
        model: &str,
        key: Option<String>,
        mode: &'static str,
        timeout: Duration,
    ) -> Result<Self, BackendError> {
        let client = Client::new(&format!("FastCapture/{}", env!("CARGO_PKG_VERSION")))
            .map_err(|error| backend::from_http(error, "the model server"))?;
        Ok(Self {
            client,
            endpoint: normalize(endpoint),
            model: model.to_owned(),
            key,
            mode,
            timeout,
            schema_in_prompt: AtomicBool::new(false),
        })
    }

    fn service(&self) -> &'static str {
        if self.mode == "local" {
            "The local model server"
        } else {
            "The model server"
        }
    }

    /// The request body for `request`.
    pub fn body(&self, request: &NoteRequest, schema_in_prompt: bool) -> Value {
        let mut content: Vec<Value> = request
            .images
            .iter()
            .map(|image| {
                json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:{};base64,{}", image.media_type, image.base64())}
                })
            })
            .collect();
        content.push(json!({"type": "text", "text": request.user_text}));
        let system = if schema_in_prompt {
            format!(
                "{}\n\nThe JSON schema the answer must match:\n{}",
                request.system,
                schema::schema()
            )
        } else {
            request.system.to_owned()
        };
        let mut body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": content}
            ],
            "stream": false
        });
        if !schema_in_prompt {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {"name": schema::NAME, "strict": true, "schema": schema::schema()}
            });
        }
        body
    }

    fn send(&self, body: &Value) -> Result<crate::net::http::Response, BackendError> {
        let body = body.to_string();
        let mut headers = vec![("content-type", "application/json".to_owned())];
        if let Some(key) = &self.key {
            headers.push(("authorization", format!("Bearer {key}")));
        }
        self.client
            .send(&Request {
                method: "POST",
                url: &format!("{}/v1/chat/completions", self.endpoint),
                headers,
                body: body.as_bytes(),
                timeout: self.timeout,
            })
            .map_err(|error| backend::from_http(error, self.service()))
    }
}

impl Backend for OpenAi {
    fn label(&self) -> String {
        format!("{}/{}", self.mode, self.model)
    }

    fn complete(&self, request: &NoteRequest) -> Result<NoteJson, BackendError> {
        let schema_in_prompt = self.schema_in_prompt.load(Ordering::Relaxed);
        let mut response = self.send(&self.body(request, schema_in_prompt))?;
        if response.status == 400 && !schema_in_prompt {
            let message = backend::error_message(&response).to_lowercase();
            if message.contains("response_format") || message.contains("json_schema") {
                crate::log::line("openai: the server rejected response_format; using the prompt");
                self.schema_in_prompt.store(true, Ordering::Relaxed);
                response = self.send(&self.body(request, true))?;
            }
        }
        if response.status != 200 {
            return Err(backend::from_status(&response, self.service()));
        }
        let reply: Value =
            serde_json::from_slice(&response.body).map_err(|error| BackendError::Retry {
                after: None,
                reason: format!("{} sent a reply that isn't JSON: {error}", self.service()),
            })?;
        let choice = &reply["choices"][0];
        if choice["finish_reason"].as_str() == Some("length") {
            return Err(BackendError::Fail(format!(
                "{}'s note was cut off at the length limit.",
                self.service()
            )));
        }
        let text = choice["message"]["content"].as_str().ok_or_else(|| {
            BackendError::Invalid(reply::ReplyError(vec!["the reply has no text".to_owned()]))
        })?;
        reply::parse(text).map_err(BackendError::Invalid)
    }
}

/// The model ids the server at `endpoint` lists (`GET /v1/models`).
pub fn list_models(endpoint: &str, timeout: Duration) -> Result<Vec<String>, BackendError> {
    let client = Client::new("FastCapture")
        .map_err(|error| backend::from_http(error, "the model server"))?;
    let response = client
        .send(&Request {
            method: "GET",
            url: &format!("{}/v1/models", normalize(endpoint)),
            headers: Vec::new(),
            body: &[],
            timeout,
        })
        .map_err(|error| backend::from_http(error, "The model server"))?;
    if response.status != 200 {
        return Err(backend::from_status(&response, "The model server"));
    }
    let value: Value = serde_json::from_slice(&response.body)
        .map_err(|_| BackendError::Fail("The model server's model list isn't JSON".to_owned()))?;
    let mut models: Vec<String> = value["data"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::images::ModelImage;
    use crate::model::request::CaptureFacts;
    use crate::net::test_server::{Scripted, TestServer};
    use std::path::PathBuf;

    const NOTE: &str = r#"{"title":"Lag","summary":"S","body":"B","key_points":[],"tasks":[],"tags":[],"links":[],"home":null}"#;

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
                captured: String::new(),
                raw: PathBuf::from(r"raw\x.md"),
                kinds: Vec::new(),
                attachments: Vec::new(),
                fallback_title: "lag".into(),
            },
        }
    }

    fn chat(content: &str, finish: &str) -> String {
        json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": finish}]})
            .to_string()
    }

    #[test]
    fn endpoints_are_normalized() {
        assert_eq!(
            normalize("http://localhost:11434/"),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize(" http://localhost:1234/v1 "),
            "http://localhost:1234"
        );
    }

    #[test]
    fn the_request_carries_the_schema_and_the_image_as_a_data_url() {
        let server = TestServer::start(vec![Scripted::json(200, &chat(NOTE, "stop"))]);
        let local = OpenAi::local(&format!("{}/v1", server.url()), "llama3.2-vision").unwrap();
        assert_eq!(local.complete(&request()).unwrap().title, "Lag");
        assert_eq!(local.label(), "local/llama3.2-vision");
        let seen = &server.requests()[0];
        assert_eq!(seen.path, "/v1/chat/completions");
        assert_eq!(
            seen.header("authorization"),
            None,
            "no key for a local server"
        );
        let body = seen.json();
        assert_eq!(
            body["response_format"]["json_schema"]["schema"],
            schema::schema()
        );
        assert_eq!(body["messages"][0]["content"], crate::model::prompt::RULES);
        assert_eq!(
            body["messages"][1]["content"][0]["image_url"]["url"],
            "data:image/png;base64,cG5n"
        );
    }

    #[test]
    fn a_server_without_response_format_gets_the_schema_in_the_prompt_from_then_on() {
        let server = TestServer::start(vec![
            Scripted::json(
                400,
                r#"{"error":{"message":"response_format type json_schema is not supported"}}"#,
            ),
            Scripted::json(200, &chat(&format!("Sure! {NOTE}"), "stop")),
            Scripted::json(200, &chat(NOTE, "stop")),
        ]);
        let local = OpenAi::local(&server.url(), "m").unwrap();
        assert_eq!(local.complete(&request()).unwrap().title, "Lag");
        assert_eq!(local.complete(&request()).unwrap().title, "Lag");
        let seen = server.requests();
        assert_eq!(seen.len(), 3);
        assert!(seen[0].json().get("response_format").is_some());
        for later in &seen[1..] {
            let body = later.json();
            assert!(body.get("response_format").is_none());
            assert!(
                body["messages"][0]["content"]
                    .as_str()
                    .unwrap()
                    .contains("The JSON schema the answer must match")
            );
        }
    }

    #[test]
    fn cutoffs_keys_and_errors() {
        let server = TestServer::start(vec![
            Scripted::json(200, &chat("{\"ti", "length")),
            Scripted::json(404, r#"{"error":"model 'nope' not found"}"#),
            Scripted::json(503, "busy"),
        ]);
        let cloud = OpenAi::new(
            &server.url(),
            "nope",
            Some("sk-test".into()),
            "cloud",
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(
            matches!(cloud.complete(&request()), Err(BackendError::Fail(m)) if m.contains("cut off"))
        );
        assert!(
            matches!(cloud.complete(&request()), Err(BackendError::Fail(m)) if m.contains("model 'nope' not found"))
        );
        assert!(matches!(
            cloud.complete(&request()),
            Err(BackendError::Retry { .. })
        ));
        assert_eq!(
            server.requests()[0].header("authorization"),
            Some("Bearer sk-test")
        );
    }

    #[test]
    fn models_are_listed_sorted() {
        let server = TestServer::start(vec![Scripted::json(
            200,
            r#"{"object":"list","data":[{"id":"qwen2.5vl"},{"id":"gemma3"}]}"#,
        )]);
        assert_eq!(
            list_models(&server.url(), Duration::from_secs(5)).unwrap(),
            ["gemma3", "qwen2.5vl"]
        );
        assert_eq!(server.requests()[0].path, "/v1/models");
    }
}
