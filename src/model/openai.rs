//! The OpenAI-compatible backend: `POST /v1/chat/completions`, as Ollama, LM Studio and Foundry
//! Local serve it on this PC, and OpenAI, OpenRouter, Azure OpenAI and others in the cloud. The
//! note schema goes as `response_format`; a server that rejects that gets the schema in the
//! system prompt instead.

use super::backend::{self, Backend, BackendError, Meter, Usage};
use super::request::NoteRequest;
use crate::net::http::{Client, Request, parse_url};
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
    /// How errors name the server: "The local model server", or the cloud API's host.
    service: String,
    /// Azure OpenAI takes the key as `api-key` rather than a bearer token.
    azure: bool,
    timeout: Duration,
    /// The server rejected `response_format` once: ask with the schema in the prompt.
    schema_in_prompt: AtomicBool,
    usage: Meter,
}

/// `endpoint` without a trailing `/` or `/v1`.
pub fn normalize(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    endpoint.strip_suffix("/v1").unwrap_or(endpoint).to_owned()
}

/// `endpoint`'s host, lowercase: what the first-use screen names and errors call the server.
pub fn host(endpoint: &str) -> Option<String> {
    parse_url(&normalize(endpoint))
        .ok()
        .map(|target| target.host.to_ascii_lowercase())
}

/// Whether `host` is Azure OpenAI's, which wants the key as `api-key`.
fn is_azure(host: &str) -> bool {
    host.ends_with(".openai.azure.com") || host.ends_with(".services.ai.azure.com")
}

/// The header that carries `key` to `endpoint`.
fn key_header(endpoint: &str, key: &str) -> (&'static str, String) {
    if host(endpoint).is_some_and(|host| is_azure(&host)) {
        ("api-key", key.to_owned())
    } else {
        ("authorization", format!("Bearer {key}"))
    }
}

impl OpenAi {
    /// A local server: 5 minutes per request, no key.
    pub fn local(endpoint: &str, model: &str) -> Result<Self, BackendError> {
        Self::new(endpoint, model, None, "local", Duration::from_secs(300))
    }

    /// A cloud API: `https://` only, since the key goes with every request; 2 minutes per
    /// request.
    pub fn cloud(endpoint: &str, model: &str, key: String) -> Result<Self, BackendError> {
        match parse_url(&normalize(endpoint)) {
            Ok(target) if target.secure => {}
            _ => {
                return Err(BackendError::Fail(format!(
                    "{} isn't an https:// address. A cloud API's key is only sent over https.",
                    endpoint.trim()
                )));
            }
        }
        Self::new(
            endpoint,
            model,
            Some(key),
            "cloud",
            Duration::from_secs(120),
        )
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
        let host = host(endpoint);
        let service = match (mode, &host) {
            ("local", _) => "The local model server".to_owned(),
            (_, Some(host)) => host.clone(),
            (_, None) => "The model server".to_owned(),
        };
        Ok(Self {
            client,
            azure: host.as_deref().is_some_and(is_azure),
            service,
            endpoint: normalize(endpoint),
            model: model.to_owned(),
            key,
            mode,
            timeout,
            schema_in_prompt: AtomicBool::new(false),
            usage: Meter::default(),
        })
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
            headers.push(if self.azure {
                ("api-key", key.clone())
            } else {
                ("authorization", format!("Bearer {key}"))
            });
        }
        self.client
            .send(&Request {
                method: "POST",
                url: &format!("{}/v1/chat/completions", self.endpoint),
                headers,
                body: body.as_bytes(),
                timeout: self.timeout,
            })
            .map_err(|error| backend::from_http(error, self.service.as_str()))
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
        if response.status == 400 && !request.images.is_empty() {
            let message = backend::error_message(&response).to_lowercase();
            if message.contains("image") || message.contains("vision") {
                return Err(BackendError::Fail(format!(
                    "{} doesn't read images. Choose a model that does in Settings.",
                    self.model
                )));
            }
        }
        if response.status != 200 {
            return Err(backend::from_status(&response, self.service.as_str()));
        }
        let reply: Value =
            serde_json::from_slice(&response.body).map_err(|error| BackendError::Retry {
                after: None,
                reason: format!(
                    "{} sent a reply that isn't JSON: {error}",
                    self.service.as_str()
                ),
            })?;
        if let Some(usage) = usage_of(&reply) {
            crate::log::line(&format!(
                "openai: {} input ({} cached), {} output tokens",
                usage.input, usage.cache_read, usage.output
            ));
            self.usage.add(usage);
        }
        let choice = &reply["choices"][0];
        if choice["finish_reason"].as_str() == Some("length") {
            return Err(BackendError::Fail(format!(
                "{}'s note was cut off at the length limit.",
                self.service.as_str()
            )));
        }
        if choice["message"]["refusal"]
            .as_str()
            .is_some_and(|refusal| !refusal.trim().is_empty())
        {
            return Err(BackendError::Fail(
                "The model declined to write a note for this capture.".to_owned(),
            ));
        }
        let text = choice["message"]["content"].as_str().ok_or_else(|| {
            BackendError::Invalid(reply::ReplyError(vec!["the reply has no text".to_owned()]))
        })?;
        reply::parse(text).map_err(BackendError::Invalid)
    }

    fn take_usage(&self) -> Option<Usage> {
        self.usage.take()
    }
}

/// A reply's `usage`: `prompt_tokens` includes the cached ones, which are counted apart.
fn usage_of(reply: &Value) -> Option<Usage> {
    let usage = reply.get("usage").filter(|usage| usage.is_object())?;
    let count = |value: &Value| value.as_u64().unwrap_or(0);
    let prompt = count(&usage["prompt_tokens"]);
    let cached = count(&usage["prompt_tokens_details"]["cached_tokens"]).min(prompt);
    Some(Usage {
        input: prompt - cached,
        cache_read: cached,
        cache_write: 0,
        output: count(&usage["completion_tokens"]),
    })
}

/// The model ids the server at `endpoint` lists (`GET /v1/models`), sending `key` when
/// there is one.
pub fn list_models(
    endpoint: &str,
    key: Option<&str>,
    timeout: Duration,
) -> Result<Vec<String>, BackendError> {
    let client = Client::new("FastCapture")
        .map_err(|error| backend::from_http(error, "the model server"))?;
    let response = client
        .send(&Request {
            method: "GET",
            url: &format!("{}/v1/models", normalize(endpoint)),
            headers: key
                .map(|key| key_header(endpoint, key))
                .into_iter()
                .collect(),
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
                links: Vec::new(),
                truncated: Vec::new(),
                candidates: Vec::new(),
                home_notes: Vec::new(),
                home_folders: Vec::new(),
            },
        }
    }

    fn chat(content: &str, finish: &str) -> String {
        json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": finish}]})
            .to_string()
    }

    fn chat_with_usage(content: &str, usage: Value) -> String {
        let mut reply: Value = serde_json::from_str(&chat(content, "stop")).unwrap();
        reply["usage"] = usage;
        reply.to_string()
    }

    #[test]
    fn usage_is_read_from_replies_and_added_up_until_taken() {
        let server = TestServer::start(vec![
            Scripted::json(
                200,
                &chat_with_usage(
                    "not a note",
                    json!({"prompt_tokens": 1000, "completion_tokens": 50,
                           "prompt_tokens_details": {"cached_tokens": 600}}),
                ),
            ),
            Scripted::json(
                200,
                &chat_with_usage(
                    NOTE,
                    json!({"prompt_tokens": 400, "completion_tokens": 120}),
                ),
            ),
            Scripted::json(200, &chat(NOTE, "stop")),
        ]);
        let cloud = OpenAi::new(
            &server.url(),
            "m",
            Some("sk-test".into()),
            "cloud",
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(matches!(
            cloud.complete(&request()),
            Err(BackendError::Invalid(_))
        ));
        assert_eq!(cloud.complete(&request()).unwrap().title, "Lag");
        assert_eq!(
            cloud.take_usage(),
            Some(Usage {
                input: 800,
                cache_read: 600,
                cache_write: 0,
                output: 170
            })
        );
        assert_eq!(cloud.take_usage(), None, "taken");
        cloud.complete(&request()).unwrap();
        assert_eq!(
            cloud.take_usage(),
            None,
            "a reply without usage counts nothing"
        );
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
    fn a_cloud_api_must_be_https_and_is_named_by_its_host() {
        assert!(
            matches!(OpenAi::cloud("http://api.openai.com/v1", "m", "k".into()), Err(BackendError::Fail(m)) if m.contains("https://"))
        );
        assert!(OpenAi::cloud("api.openai.com", "m", "k".into()).is_err());
        let cloud = OpenAi::cloud(
            "https://OpenRouter.ai/api/v1/",
            "openai/gpt-5-mini",
            "k".into(),
        )
        .unwrap();
        assert_eq!(cloud.endpoint, "https://OpenRouter.ai/api");
        assert_eq!(cloud.service, "openrouter.ai");
        assert!(!cloud.azure);
        assert_eq!(cloud.label(), "cloud/openai/gpt-5-mini");
        let azure = OpenAi::cloud(
            "https://notes.openai.azure.com/openai/v1",
            "gpt-5-mini",
            "k".into(),
        )
        .unwrap();
        assert!(azure.azure);
        assert_eq!(
            key_header("https://notes.services.ai.azure.com/openai/v1", "k"),
            ("api-key", "k".to_owned())
        );
        assert_eq!(
            key_header("https://api.openai.com/v1", "k"),
            ("authorization", "Bearer k".to_owned())
        );
        assert_eq!(
            host("https://API.openai.com/v1"),
            Some("api.openai.com".to_owned())
        );
    }

    #[test]
    fn errors_name_the_host_and_say_what_to_do() {
        let server = TestServer::start(vec![
            Scripted::json(401, r#"{"error":{"message":"Incorrect API key provided"}}"#),
            Scripted::json(
                400,
                r#"{"error":{"message":"Invalid content type. image_url is only supported by certain models."}}"#,
            ),
            Scripted::json(
                200,
                &json!({"choices": [{"message": {"role": "assistant", "content": null,
                    "refusal": "I can't help with that."}, "finish_reason": "stop"}]})
                .to_string(),
            ),
            Scripted::json(429, r#"{"error":{"message":"Rate limit reached"}}"#)
                .with_header("retry-after", "20"),
        ]);
        let cloud = OpenAi::new(
            &server.url(),
            "text-only",
            Some("sk-test".into()),
            "cloud",
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(
            matches!(cloud.complete(&request()), Err(BackendError::Pause(m)) if m.starts_with("127.0.0.1 refused the API key"))
        );
        assert!(
            matches!(cloud.complete(&request()), Err(BackendError::Fail(m)) if m == "text-only doesn't read images. Choose a model that does in Settings.")
        );
        assert!(
            matches!(cloud.complete(&request()), Err(BackendError::Fail(m)) if m.contains("declined"))
        );
        assert_eq!(
            cloud.complete(&request()),
            Err(BackendError::Retry {
                after: Some(Duration::from_secs(20)),
                reason: "127.0.0.1 is rate limiting requests: Rate limit reached".to_owned()
            })
        );
    }

    #[test]
    fn models_are_listed_sorted() {
        let server = TestServer::start(vec![Scripted::json(
            200,
            r#"{"object":"list","data":[{"id":"qwen2.5vl"},{"id":"gemma3"}]}"#,
        )]);
        assert_eq!(
            list_models(&server.url(), None, Duration::from_secs(5)).unwrap(),
            ["gemma3", "qwen2.5vl"]
        );
        assert_eq!(server.requests()[0].path, "/v1/models");
        assert_eq!(server.requests()[0].header("authorization"), None);
    }

    #[test]
    fn a_cloud_model_list_carries_the_key() {
        let server = TestServer::start(vec![
            Scripted::json(200, r#"{"data":[{"id":"gpt-5-mini"}]}"#),
            Scripted::json(401, r#"{"error":{"message":"Incorrect API key"}}"#),
        ]);
        assert_eq!(
            list_models(&server.url(), Some("sk-test"), Duration::from_secs(5)).unwrap(),
            ["gpt-5-mini"]
        );
        assert_eq!(
            server.requests()[0].header("authorization"),
            Some("Bearer sk-test")
        );
        assert!(matches!(
            list_models(&server.url(), Some("bad"), Duration::from_secs(5)),
            Err(BackendError::Pause(_))
        ));
    }
}
