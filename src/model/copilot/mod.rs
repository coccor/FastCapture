//! GitHub Copilot through the user's own installed Copilot CLI, driven as the Copilot SDKs
//! drive it: one session per note, with no tools, Copilot's own prompt replaced by the writing
//! rules, the images as attachments and the note's schema as the reply's format. Usage counts
//! against the user's Copilot plan; a used-up allowance pauses the queue.

pub mod cli;
pub mod rpc;

use super::backend::{Backend, BackendError, Meter, Usage};
use super::request::NoteRequest;
use crate::model::claude_code::when;
use crate::note::{NoteJson, reply, schema};
use cli::{Server, StartError};
use rpc::RpcError;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Copilot chooses the model; the only one Copilot Free offers.
pub const DEFAULT_MODEL: &str = "auto";

const SERVICE: &str = "GitHub Copilot";
/// Starting the CLI and connecting to it.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// Each call that answers at once (everything but the note itself).
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// A whole note, from starting the CLI.
const TIMEOUT: Duration = Duration::from_secs(300);
/// How long a used-up allowance pauses the queue, since Copilot doesn't say when it resets.
const LIMIT_WAIT: Duration = Duration::from_secs(30 * 60);
const SIGN_IN: &str = "Run copilot login in a terminal.";

pub struct Copilot {
    path: PathBuf,
    model: String,
    /// The empty folder it runs in.
    dir: PathBuf,
    meter: Meter,
}

impl Copilot {
    /// The Copilot CLI at `path` (or found as `cli::find` finds it), with `model`.
    pub fn new(path: Option<&Path>, model: &str) -> Result<Self, BackendError> {
        let found = cli::find(path).ok_or_else(|| {
            BackendError::Pause(match path {
                Some(path) => format!(
                    "The Copilot CLI isn't at {}. Check its path in Settings.",
                    path.display()
                ),
                None => "The Copilot CLI isn't installed, or isn't on PATH. Install it \
                         (winget install GitHub.Copilot, or npm install -g @github/copilot), \
                         then run copilot login."
                    .to_owned(),
            })
        })?;
        Ok(Self::with_path(found, model, cli::scratch_dir()))
    }

    pub fn with_path(path: PathBuf, model: &str, dir: PathBuf) -> Self {
        let model = model.trim();
        Self {
            path,
            model: if model.is_empty() {
                DEFAULT_MODEL
            } else {
                model
            }
            .to_owned(),
            dir,
            meter: Meter::default(),
        }
    }

    /// Sends the message and reads the session's events until it is done.
    fn converse(
        &self,
        server: &mut Server,
        session: &str,
        request: &NoteRequest,
        started: Instant,
    ) -> Result<Reply, BackendError> {
        match server.call(
            "session.send",
            send_params(session, request, false),
            CALL_TIMEOUT,
        ) {
            Err(RpcError::Remote { message, .. }) if mentions_schema(&message) => {
                crate::log::line(&format!(
                    "copilot: no reply format ({message}), schema in the prompt"
                ));
                server
                    .call(
                        "session.send",
                        send_params(session, request, true),
                        CALL_TIMEOUT,
                    )
                    .map_err(|error| from_rpc(error, server))?;
            }
            sent => {
                sent.map_err(|error| from_rpc(error, server))?;
            }
        }
        let mut reply = Reply::default();
        let deadline = started + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let _ = server.call("session.abort", json!({"sessionId": session}), CALL_TIMEOUT);
                return Err(BackendError::Retry {
                    after: None,
                    reason: format!("{SERVICE} didn't finish within 5 minutes."),
                });
            }
            match server.notification(left) {
                Ok(notification) => {
                    let params = &notification["params"];
                    if notification["method"] == "session.event"
                        && params["sessionId"] == session
                        && reply.take(&params["event"])
                    {
                        return Ok(reply);
                    }
                }
                Err(RpcError::TimedOut) => {}
                Err(error) => return Err(from_rpc(error, server)),
            }
        }
    }
}

/// What Settings shows about the Copilot CLI: a line saying which one is used and whether it
/// is signed in, and the models the plan offers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Status {
    pub line: String,
    pub models: Vec<String>,
}

/// Finds the Copilot CLI, starts it and asks who is signed in, which models the plan offers and
/// how many chat requests are left. Sends no prompt, but takes a second or two, so call it off
/// the UI thread.
pub fn status(path: Option<&Path>) -> Status {
    let Some(found) = cli::find(path) else {
        return Status {
            line: match path {
                Some(path) => format!("The Copilot CLI isn't at {}.", path.display()),
                None => "The Copilot CLI isn't installed, or isn't on PATH: install it with \
                         winget install GitHub.Copilot, then run copilot login."
                    .to_owned(),
            },
            models: Vec::new(),
        };
    };
    let dir = cli::scratch_dir();
    let _ = std::fs::create_dir_all(&dir);
    let server = match Server::start(&found, &dir, START_TIMEOUT) {
        Ok(server) => server,
        Err(error) => {
            return Status {
                line: sentence(&error.to_string()),
                models: Vec::new(),
            };
        }
    };
    let ask = |method: &str| {
        server
            .call(method, json!({}), CALL_TIMEOUT)
            .unwrap_or(Value::Null)
    };
    let auth = ask("auth.getStatus");
    let models: Vec<String> = if auth["isAuthenticated"] == true {
        ask("models.list")["models"]
            .as_array()
            .map(|models| {
                models
                    .iter()
                    .filter_map(|model| model["id"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let quota = if models.is_empty() {
        Value::Null
    } else {
        ask("account.getQuota")
    };
    let version = if server.version.is_empty() {
        String::new()
    } else {
        format!(" {}", server.version)
    };
    Status {
        line: format!(
            "Copilot CLI{version}{}. At {}.",
            account_text(&auth, &models, &quota),
            shown_path(&found).display()
        ),
        models,
    }
}

/// The path as the user knows it: for an npm install, the `copilot.cmd` npm made rather than
/// the program deep inside its package.
pub fn shown_path(found: &Path) -> PathBuf {
    let mut npm = PathBuf::new();
    for part in found.components() {
        if part.as_os_str().eq_ignore_ascii_case("node_modules") {
            return npm.join("copilot.cmd");
        }
        npm.push(part);
    }
    found.to_path_buf()
}

/// ", signed in as someone; 197 of 200 chat requests left this month", from the CLI's answers.
pub fn account_text(auth: &Value, models: &[String], quota: &Value) -> String {
    let login = auth["login"].as_str().unwrap_or("you");
    if auth["isAuthenticated"] != true {
        return format!(
            ", not signed in: {}",
            SIGN_IN.to_lowercase().trim_end_matches('.')
        );
    }
    if models.is_empty() {
        return if auth["authType"] == "gh-cli" {
            format!(
                ", signed in as {login} through gh only, which offers no models: run copilot \
                 login in a terminal"
            )
        } else {
            format!(", signed in as {login}, but your plan offers no models")
        };
    }
    let chat = &quota["quotaSnapshots"]["chat"];
    let left = match (
        chat["isUnlimitedEntitlement"].as_bool(),
        chat["entitlementRequests"].as_u64(),
        chat["usedRequests"].as_u64(),
    ) {
        (Some(false), Some(total @ 1..), Some(used)) => format!(
            "; {} of {total} chat requests left this month",
            total.saturating_sub(used)
        ),
        _ => String::new(),
    };
    format!(", signed in as {login}{left}")
}

/// The session for one note: no tools, no instructions, skills or saved state of the user's,
/// and the rules in place of Copilot's own prompt. Auto is Copilot's choice, so no model is
/// named for it.
pub fn session_params(model: &str, rules: &str, dir: &Path) -> Value {
    let mut params = json!({
        "clientName": "fastcapture",
        "workingDirectory": dir.display().to_string(),
        "availableTools": [],
        "systemMessage": {"mode": "replace", "content": rules},
        "enableConfigDiscovery": false,
        "enableSkills": false,
        "enableSessionStore": false,
        "infiniteSessions": {"enabled": false},
        "requestPermission": false,
        "streaming": false,
    });
    if !model.eq_ignore_ascii_case(DEFAULT_MODEL) {
        params["model"] = json!(model);
    }
    params
}

/// The message: the text, the images as attachments, and the schema as the reply's format, or
/// (`schema_in_prompt`, for a CLI that refuses that) written after the text.
pub fn send_params(session: &str, request: &NoteRequest, schema_in_prompt: bool) -> Value {
    let mut params = json!({"sessionId": session, "prompt": request.user_text});
    if !request.images.is_empty() {
        let attachments: Vec<Value> = request
            .images
            .iter()
            .enumerate()
            .map(|(n, image)| {
                let extension = image.media_type.rsplit('/').next().unwrap_or("png");
                json!({
                    "type": "blob",
                    "data": image.base64(),
                    "mimeType": image.media_type,
                    "displayName": format!("image-{}.{extension}", n + 1),
                })
            })
            .collect();
        params["attachments"] = json!(attachments);
    }
    if schema_in_prompt {
        params["prompt"] = json!(format!(
            "{}\n\nThe JSON schema the answer must match:\n{}",
            request.user_text,
            schema::schema()
        ));
    } else {
        params["responseFormat"] = json!({
            "type": "json_schema",
            "jsonSchema": {"name": schema::NAME, "strict": true, "schema": schema::schema()},
        });
    }
    params
}

fn mentions_schema(message: &str) -> bool {
    let message = message.to_lowercase();
    ["responseformat", "response_format", "json_schema", "schema"]
        .iter()
        .any(|word| message.contains(word))
}

/// What a session said about one note.
#[derive(Debug, Default)]
pub struct Reply {
    /// The last message's text.
    pub text: Option<String>,
    /// The model Copilot used (Auto names the one it chose).
    pub model: Option<String>,
    pub usage: Option<Usage>,
    /// The AI units Copilot counted, in billionths.
    pub nano_ai_units: u64,
    /// The `session.error` event's data, when there was one.
    pub error: Option<Value>,
}

impl Reply {
    /// Takes one of the session's events; true once the session is done with the note.
    pub fn take(&mut self, event: &Value) -> bool {
        // A sub-agent's events aren't the note; with no tools there shouldn't be any.
        if event.get("agentId").is_some_and(|agent| !agent.is_null()) {
            return false;
        }
        let data = &event["data"];
        match event["type"].as_str() {
            Some("assistant.message") => {
                if let Some(text) = data["content"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                {
                    self.text = Some(text.to_owned());
                }
                false
            }
            Some("assistant.usage") => {
                self.usage
                    .get_or_insert_with(Usage::default)
                    .add(usage_of(data));
                self.nano_ai_units += data["copilotUsage"]["totalNanoAiu"].as_u64().unwrap_or(0);
                if let Some(model) = data["model"].as_str() {
                    self.model = Some(model.to_owned());
                }
                false
            }
            Some("session.warning") => {
                crate::log::line(&format!("copilot: warning: {data}"));
                false
            }
            Some("session.error") => {
                self.error = Some(data.clone());
                true
            }
            Some("session.idle") => true,
            _ => false,
        }
    }
}

/// An `assistant.usage` event's tokens. Its input count includes the cached tokens.
fn usage_of(data: &Value) -> Usage {
    let count = |key: &str| data[key].as_u64().unwrap_or(0);
    let cache_read = count("cacheReadTokens");
    let cache_write = count("cacheWriteTokens");
    Usage {
        input: count("inputTokens").saturating_sub(cache_read + cache_write),
        cache_read,
        cache_write,
        output: count("outputTokens"),
    }
}

/// What a finished session means: a note, or what the runner should do. `now` is milliseconds
/// since 1970.
pub fn interpret(reply: &Reply, now: u64) -> Result<NoteJson, BackendError> {
    if let Some(error) = &reply.error {
        return Err(error_from(error, now));
    }
    match &reply.text {
        Some(text) => reply::parse(text).map_err(BackendError::Invalid),
        None => Err(BackendError::Invalid(reply::ReplyError(vec![
            "the reply has no text".to_owned(),
        ]))),
    }
}

/// A `session.error` event's data as what the runner should do.
pub fn error_from(data: &Value, now: u64) -> BackendError {
    let message = data["message"].as_str().unwrap_or("an error").trim();
    let kind = data["errorType"].as_str().unwrap_or("");
    let code = data["errorCode"].as_str().unwrap_or("");
    let paused = |what: &str| {
        crate::log::line(&format!("copilot: limit: {data}"));
        let until = now + LIMIT_WAIT.as_millis() as u64;
        BackendError::PauseUntil {
            until,
            reason: format!("{what} Notes carry on {}.", when(until / 1000)),
        }
    };
    match (kind, code) {
        (_, "billing_not_configured") => BackendError::Pause(format!(
            "{SERVICE} needs billing set up on github.com for this ({message})."
        )),
        ("authentication", _) => {
            BackendError::Pause(format!("{SERVICE} isn't signed in ({message}). {SIGN_IN}"))
        }
        ("authorization" | "policy", _) => BackendError::Pause(format!(
            "Your Copilot plan or organization doesn't allow this ({message})."
        )),
        ("quota", _) => paused("Your Copilot allowance is used up."),
        ("rate_limit", "user_weekly_rate_limited" | "user_global_rate_limited") => {
            paused("Copilot is limiting your account's requests.")
        }
        ("rate_limit", _) => BackendError::Retry {
            after: None,
            reason: format!("{SERVICE} is rate limiting requests: {message}"),
        },
        ("context_limit", _) => BackendError::Fail(format!(
            "The capture is too long for this model ({message})."
        )),
        _ if matches!(data["statusCode"].as_u64(), Some(429 | 500..=599)) => BackendError::Retry {
            after: None,
            reason: format!("{SERVICE} is unavailable: {message}"),
        },
        _ => BackendError::Fail(format!("{SERVICE}: {message}")),
    }
}

/// `text` with its first letter capitalised, and a full stop.
fn sentence(text: &str) -> String {
    let mut letters = text.chars();
    let first: String = letters
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    format!("{first}{}.", letters.as_str())
}

fn from_start(error: StartError) -> BackendError {
    match error {
        StartError::TimedOut => BackendError::Retry {
            after: None,
            reason: sentence(&error.to_string()),
        },
        StartError::Protocol(_) => BackendError::Pause(format!(
            "{} Update FastCapture, or the Copilot CLI.",
            sentence(&error.to_string())
        )),
        _ => BackendError::Pause(sentence(&error.to_string())),
    }
}

fn from_rpc(error: RpcError, server: &mut Server) -> BackendError {
    match error {
        RpcError::Closed => {
            let said = server.ended_with();
            BackendError::Retry {
                after: None,
                reason: format!(
                    "The Copilot CLI ended while writing the note{}",
                    if said.is_empty() {
                        ".".to_owned()
                    } else {
                        format!(": {said}")
                    }
                ),
            }
        }
        RpcError::TimedOut => BackendError::Retry {
            after: None,
            reason: "The Copilot CLI didn't answer in time.".to_owned(),
        },
        RpcError::Remote { message, .. } => BackendError::Fail(format!("{SERVICE}: {message}")),
    }
}

impl Backend for Copilot {
    fn label(&self) -> String {
        format!("cloud/copilot/{}", self.model)
    }

    fn take_usage(&self) -> Option<Usage> {
        self.meter.take()
    }

    fn complete(&self, request: &NoteRequest) -> Result<NoteJson, BackendError> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| BackendError::Fail(format!("{}: {error}", self.dir.display())))?;
        let started = Instant::now();
        let mut server = Server::start(&self.path, &self.dir, START_TIMEOUT).map_err(from_start)?;
        let auth = server
            .call("auth.getStatus", json!({}), CALL_TIMEOUT)
            .map_err(|error| from_rpc(error, &mut server))?;
        if auth["isAuthenticated"].as_bool() == Some(false) {
            return Err(BackendError::Pause(format!(
                "{SERVICE} isn't signed in. {SIGN_IN}"
            )));
        }
        let created = server
            .call(
                "session.create",
                session_params(&self.model, request.system, &self.dir),
                CALL_TIMEOUT,
            )
            .map_err(|error| match error {
                RpcError::Remote { message, .. } if message.to_lowercase().contains("model") => {
                    BackendError::Pause(format!(
                        "{SERVICE} doesn't offer the model {} on your plan ({message}). Choose \
                         another in Settings.",
                        self.model
                    ))
                }
                error => from_rpc(error, &mut server),
            })?;
        let session = created["sessionId"]
            .as_str()
            .ok_or_else(|| BackendError::Fail(format!("{SERVICE} started no session.")))?
            .to_owned();
        let reply = self.converse(&mut server, &session, request, started);
        let _ = server.call(
            "session.delete",
            json!({"sessionId": session}),
            CALL_TIMEOUT,
        );
        drop(server);
        let reply = reply?;
        if let Some(usage) = reply.usage {
            crate::log::line(&format!(
                "copilot: {}, {} input ({} cached), {} output tokens, {:.3} AI units, {:.1} s",
                reply.model.as_deref().unwrap_or(&self.model),
                usage.input + usage.cache_write,
                usage.cache_read,
                usage.output,
                reply.nano_ai_units as f64 / 1e9,
                started.elapsed().as_secs_f64()
            ));
            self.meter.add(usage);
        }
        interpret(&reply, crate::runner::process::now_ms())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::claude_code::cli::tests::Temp;
    use crate::model::images::ModelImage;
    use crate::model::request::CaptureFacts;
    use cli::tests::{connects, fake, received};

    const NOTE: &str = r#"{"title":"Lag","summary":"S","body":"B","key_points":[],"tasks":[],"tags":["kafka"],"links":[],"home":null}"#;
    const NOW: u64 = 1_792_000_000_000;

    fn request(images: usize) -> NoteRequest {
        NoteRequest {
            system: crate::model::prompt::RULES,
            user_text: "<your_note>\nlag\n</your_note>".to_owned(),
            images: (0..images)
                .map(|_| ModelImage {
                    media_type: "image/png",
                    data: b"png".to_vec(),
                    width: 1,
                    height: 1,
                })
                .collect(),
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

    /// `assistant.usage` as the screenshot check got it, cut down.
    fn usage_event() -> Value {
        json!({"type": "assistant.usage", "data": {
            "model": "gpt-6-luna", "inputTokens": 1760, "outputTokens": 297, "cacheReadTokens": 0,
            "cacheWriteTokens": 1757, "reasoningTokens": 121, "cost": 1,
            "copilotUsage": {"totalNanoAiu": 36_842_500}
        }})
    }

    fn message_event(text: &str) -> Value {
        json!({"type": "assistant.message", "data": {"messageId": "m", "content": text, "toolRequests": []}})
    }

    #[test]
    fn a_session_has_no_tools_and_the_rules_in_place_of_copilot_s_prompt() {
        let params = session_params("auto", "Rules.", Path::new(r"C:\Local\copilot"));
        assert_eq!(params["availableTools"], json!([]));
        assert_eq!(
            params["systemMessage"],
            json!({"mode": "replace", "content": "Rules."})
        );
        assert_eq!(params["workingDirectory"], r"C:\Local\copilot");
        for off in [
            "enableConfigDiscovery",
            "enableSkills",
            "enableSessionStore",
            "requestPermission",
        ] {
            assert_eq!(params[off], false, "{off}");
        }
        assert_eq!(params["infiniteSessions"]["enabled"], false);
        assert!(params.get("model").is_none(), "Auto is Copilot's choice");
        assert_eq!(
            session_params("gpt-5.4", "R", Path::new("x"))["model"],
            "gpt-5.4"
        );
    }

    #[test]
    fn the_message_has_the_images_as_attachments_and_the_schema_as_its_format() {
        let params = send_params("s1", &request(1), false);
        assert_eq!(params["sessionId"], "s1");
        assert_eq!(params["prompt"], "<your_note>\nlag\n</your_note>");
        assert_eq!(
            params["attachments"],
            json!([{"type": "blob", "data": "cG5n", "mimeType": "image/png", "displayName": "image-1.png"}])
        );
        assert_eq!(params["responseFormat"]["type"], "json_schema");
        assert_eq!(params["responseFormat"]["jsonSchema"]["name"], schema::NAME);
        assert_eq!(
            params["responseFormat"]["jsonSchema"]["schema"],
            schema::schema()
        );

        let fallback = send_params("s1", &request(0), true);
        assert!(fallback.get("attachments").is_none());
        assert!(fallback.get("responseFormat").is_none());
        assert!(
            fallback["prompt"]
                .as_str()
                .unwrap()
                .contains(r#""required":["title""#)
        );
        assert!(mentions_schema("Unknown field responseFormat"));
        assert!(!mentions_schema("Unhandled method session.send"));
    }

    #[test]
    fn a_session_s_events_give_the_note_and_its_usage() {
        let mut reply = Reply::default();
        assert!(!reply.take(&json!({"type": "session.usage_info", "data": {"systemTokens": 484}})));
        assert!(!reply.take(&usage_event()));
        assert!(!reply.take(
            &json!({"type": "assistant.message", "agentId": "sub", "data": {"content": "not this"}})
        ));
        assert!(!reply.take(&message_event(NOTE)));
        assert!(reply.take(&json!({"type": "session.idle", "data": {}})));
        assert_eq!(
            reply.usage,
            Some(Usage {
                input: 3,
                cache_read: 0,
                cache_write: 1757,
                output: 297
            }),
            "the input count includes what was cached"
        );
        assert_eq!(reply.nano_ai_units, 36_842_500);
        assert_eq!(reply.model.as_deref(), Some("gpt-6-luna"));
        assert_eq!(interpret(&reply, NOW).unwrap().title, "Lag");

        let empty = Reply::default();
        assert!(matches!(
            interpret(&empty, NOW),
            Err(BackendError::Invalid(_))
        ));
        let mut prose = Reply::default();
        prose.take(&message_event("Here is your note!"));
        assert!(matches!(
            interpret(&prose, NOW),
            Err(BackendError::Invalid(_))
        ));
    }

    fn error(kind: &str, code: Option<&str>) -> BackendError {
        let mut data = json!({"errorType": kind, "message": "It said no"});
        if let Some(code) = code {
            data["errorCode"] = json!(code);
        }
        let mut reply = Reply::default();
        assert!(reply.take(&json!({"type": "session.error", "data": data})));
        interpret(&reply, NOW).unwrap_err()
    }

    #[test]
    fn errors_pause_retry_or_fail_by_their_type() {
        let BackendError::PauseUntil { until, reason } = error("quota", Some("quota_exceeded"))
        else {
            panic!()
        };
        assert_eq!(until, NOW + 30 * 60 * 1000);
        assert!(reason.contains("allowance is used up"), "{reason}");
        assert!(matches!(
            error("rate_limit", Some("user_weekly_rate_limited")),
            BackendError::PauseUntil { .. }
        ));
        assert!(matches!(
            error("rate_limit", Some("rate_limited")),
            BackendError::Retry { .. }
        ));
        let BackendError::Pause(signed_out) = error("authentication", None) else {
            panic!()
        };
        assert!(signed_out.contains("copilot login"), "{signed_out}");
        assert!(matches!(error("policy", None), BackendError::Pause(_)));
        assert!(matches!(
            error("authorization", None),
            BackendError::Pause(_)
        ));
        assert!(matches!(
            error("quota", Some("billing_not_configured")),
            BackendError::Pause(_)
        ));
        assert!(matches!(
            error("context_limit", None),
            BackendError::Fail(_)
        ));
        assert_eq!(
            error("query", None),
            BackendError::Fail("GitHub Copilot: It said no".to_owned())
        );
        let mut reply = Reply::default();
        reply.take(&json!({"type": "session.error", "data": {"errorType": "query", "message": "Bad gateway", "statusCode": 502}}));
        assert!(matches!(
            interpret(&reply, NOW),
            Err(BackendError::Retry { .. })
        ));
    }

    /// A fake CLI that answers a whole session; `events` come after `session.send`'s reply.
    fn session(temp: &Temp, signed_in: bool, events: &[Value]) -> Copilot {
        let event = |event: &Value| {
            json!({"jsonrpc": "2.0", "method": "session.event", "params": {"sessionId": "s1", "event": event}})
                .to_string()
        };
        let mut send = String::from("session.send\t\"result\":{\"messageId\":\"m1\"}");
        for line in events {
            send.push('\t');
            send.push_str(&event(line));
        }
        let path = fake(
            temp,
            &[
                connects(),
                format!(
                    r#"auth.getStatus	"result":{{"isAuthenticated":{signed_in},"login":"someone"}}"#
                ),
                r#"session.create	"result":{"sessionId":"s1"}"#.to_owned(),
                send,
                r#"session.delete	"result":{"success":true}"#.to_owned(),
            ],
        );
        Copilot::with_path(path, "auto", temp.0.join("work"))
    }

    #[test]
    fn a_note_is_written_through_a_fake_cli() {
        let temp = Temp::new("copilot-note");
        let copilot = session(
            &temp,
            true,
            &[
                json!({"type": "session.usage_info", "data": {}}),
                usage_event(),
                message_event(NOTE),
                json!({"type": "session.idle", "data": {}}),
            ],
        );
        assert_eq!(copilot.label(), "cloud/copilot/auto");
        assert!(copilot.metered());
        let note = copilot.complete(&request(1)).unwrap();
        assert_eq!(note.title, "Lag");
        assert_eq!(copilot.take_usage().unwrap().output, 297);
        assert_eq!(copilot.take_usage(), None, "taken once");

        let got = received(&temp);
        let methods: Vec<&str> = got.iter().map(|m| m["method"].as_str().unwrap()).collect();
        assert_eq!(
            methods,
            [
                "connect",
                "auth.getStatus",
                "session.create",
                "session.send",
                "session.delete"
            ]
        );
        assert_eq!(
            got[2]["params"]["systemMessage"]["content"],
            crate::model::prompt::RULES
        );
        assert_eq!(got[2]["params"]["availableTools"], json!([]));
        assert_eq!(got[3]["params"]["attachments"][0]["data"], "cG5n");
        assert_eq!(got[4]["params"]["sessionId"], "s1");
    }

    #[test]
    fn a_signed_out_cli_or_a_used_up_allowance_pauses_the_queue() {
        let temp = Temp::new("copilot-signed-out");
        let copilot = session(&temp, false, &[]);
        let Err(BackendError::Pause(reason)) = copilot.complete(&request(0)) else {
            panic!()
        };
        assert!(reason.contains("copilot login"), "{reason}");
        assert_eq!(received(&temp).len(), 2, "no session is started");

        let temp = Temp::new("copilot-quota");
        let copilot = session(
            &temp,
            true,
            &[
                json!({"type": "session.error", "data": {"errorType": "quota", "errorCode": "quota_exceeded", "message": "You've used up your allowance"}}),
            ],
        );
        assert!(matches!(
            copilot.complete(&request(0)),
            Err(BackendError::PauseUntil { .. })
        ));
        assert_eq!(
            received(&temp).last().unwrap()["method"],
            "session.delete",
            "the session is deleted all the same"
        );
    }

    #[test]
    fn the_status_line_says_who_is_signed_in_and_what_is_left() {
        let auth = json!({"isAuthenticated": true, "authType": "user", "login": "someone"});
        let quota = json!({"quotaSnapshots": {"chat": {"isUnlimitedEntitlement": false, "entitlementRequests": 200, "usedRequests": 5}}});
        let auto = ["auto".to_owned()];
        assert_eq!(
            account_text(&auth, &auto, &quota),
            ", signed in as someone; 195 of 200 chat requests left this month"
        );
        let unlimited = json!({"quotaSnapshots": {"chat": {"isUnlimitedEntitlement": true}}});
        assert_eq!(
            account_text(&auth, &auto, &unlimited),
            ", signed in as someone"
        );
        let through_gh = json!({"isAuthenticated": true, "authType": "gh-cli", "login": "someone"});
        assert!(account_text(&through_gh, &[], &Value::Null).contains("through gh only"));
        assert_eq!(
            account_text(&json!({"isAuthenticated": false}), &[], &Value::Null),
            ", not signed in: run copilot login in a terminal"
        );

        let temp = Temp::new("copilot-status");
        let path = fake(
            &temp,
            &[
                connects(),
                r#"auth.getStatus	"result":{"isAuthenticated":true,"login":"someone"}"#.to_owned(),
                r#"models.list	"result":{"models":[{"id":"auto","name":"Auto"},{"id":"gpt-5.4","name":"GPT-5.4"}]}"#.to_owned(),
            ],
        );
        assert_eq!(
            status(Some(&path)),
            Status {
                line: format!(
                    "Copilot CLI 1.0.100, signed in as someone. At {}.",
                    path.display()
                ),
                models: vec!["auto".to_owned(), "gpt-5.4".to_owned()],
            },
            "no quota known, so none is shown"
        );
        assert!(
            status(Some(&temp.0.join("nope.exe")))
                .line
                .starts_with("The Copilot CLI isn't at")
        );
        assert_eq!(
            shown_path(Path::new(
                r"C:\Users\a\AppData\Roaming\npm\node_modules\@github\copilot\node_modules\@github\copilot-win32-x64\copilot.exe"
            )),
            Path::new(r"C:\Users\a\AppData\Roaming\npm\copilot.cmd")
        );
        assert_eq!(
            shown_path(Path::new(r"C:\Tools\copilot.exe")),
            Path::new(r"C:\Tools\copilot.exe")
        );
    }

    #[test]
    fn a_missing_cli_pauses_saying_how_to_install_it() {
        let temp = Temp::new("copilot-missing");
        let Err(BackendError::Pause(reason)) =
            Copilot::new(Some(&temp.0.join("copilot.exe")), "auto")
        else {
            panic!()
        };
        assert!(reason.contains("isn't at"), "{reason}");
        let copilot = Copilot::with_path(temp.0.join("nope.exe"), " ", temp.0.clone());
        assert_eq!(
            copilot.label(),
            "cloud/copilot/auto",
            "an empty model is Auto"
        );
        assert!(matches!(
            copilot.complete(&request(0)),
            Err(BackendError::Pause(_))
        ));
    }
}
