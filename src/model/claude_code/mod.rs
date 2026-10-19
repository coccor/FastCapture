//! The Claude Code backend: the user's own installed `claude`, run once per note with no tools,
//! signed in with the user's Claude plan. The request goes in as one stream-json user message
//! (images first, then the text); the note comes back on the stream's last `result` line, as
//! structured output when the CLI takes a schema. A plan's usage limit pauses the queue until it
//! resets.

pub mod cli;
pub mod run;

use super::backend::{Backend, BackendError};
use super::request::NoteRequest;
use crate::note::{NoteJson, reply, schema};
use cli::{Cli, SystemPrompt};
use run::RunError;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Claude Code's model aliases, as Settings offers them.
pub const MODELS: [(&str, &str); 3] = [("sonnet", "Sonnet"), ("opus", "Opus"), ("haiku", "Haiku")];
pub const DEFAULT_MODEL: &str = "sonnet";

const SERVICE: &str = "Claude Code";
/// Claude Code starts, reads the request and answers; a long capture can take a while.
const TIMEOUT: Duration = Duration::from_secs(300);
/// How long a usage limit pauses the queue when the CLI doesn't say when it resets.
const LIMIT_WAIT: Duration = Duration::from_secs(30 * 60);
const RULES_FILE: &str = "rules.md";

pub struct ClaudeCode {
    cli: Cli,
    model: String,
    /// The empty folder it runs in.
    dir: PathBuf,
}

impl ClaudeCode {
    /// The Claude Code at `path` (or found as `cli::find` finds it), with `model`.
    pub fn new(path: Option<&Path>, model: &str) -> Result<Self, BackendError> {
        let found = cli::find(path).ok_or_else(|| {
            BackendError::Pause(match path {
                Some(path) => format!(
                    "Claude Code isn't at {}. Check its path in Settings.",
                    path.display()
                ),
                None => "Claude Code isn't installed, or FastCapture can't find it. Install it, \
                         or set its path in Settings."
                    .to_owned(),
            })
        })?;
        let cli = cli::probe(&found).map_err(BackendError::Pause)?;
        Ok(Self::with_cli(cli, model, cli::scratch_dir()))
    }

    pub fn with_cli(cli: Cli, model: &str, dir: PathBuf) -> Self {
        Self {
            cli,
            model: model.to_owned(),
            dir,
        }
    }

    /// The version found, for Settings.
    pub fn version(&self) -> &str {
        &self.cli.version
    }

    /// The writing rules, with the schema when the CLI can't be given it.
    fn rules(&self, request: &NoteRequest) -> String {
        if self.cli.options.json_schema {
            request.system.to_owned()
        } else {
            format!(
                "{}\n\nThe JSON schema the answer must match:\n{}",
                request.system,
                schema::schema()
            )
        }
    }
}

/// The request as one stream-json line: the images, then the text.
pub fn message(request: &NoteRequest) -> String {
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
    let mut line =
        json!({"type": "user", "message": {"role": "user", "content": content}}).to_string();
    line.push('\n');
    line
}

/// Whether `claude auth status` says it is signed in, and with which plan (`Max`).
pub fn signed_in_from(text: &str) -> Option<(bool, Option<String>)> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    let signed_in = value["loggedIn"].as_bool()?;
    let plan = value["subscriptionType"].as_str().map(|plan| {
        let mut letters = plan.chars();
        letters
            .next()
            .map(|first| first.to_uppercase().chain(letters).collect())
            .unwrap_or_default()
    });
    Some((signed_in, plan))
}

/// A line for Settings: which Claude Code is used, and whether it is signed in. Runs it two or
/// three times, so call it off the UI thread.
pub fn status(path: Option<&Path>) -> String {
    let Some(found) = cli::find(path) else {
        return match path {
            Some(path) => format!("Claude Code isn't at {}.", path.display()),
            None => "Claude Code isn't installed, or isn't on PATH. Install it, then sign in."
                .to_owned(),
        };
    };
    let cli = match cli::probe(&found) {
        Ok(cli) => cli,
        Err(error) => return format!("{error}."),
    };
    let dir = cli::scratch_dir();
    let _ = std::fs::create_dir_all(&dir);
    let signed_in = run::run(
        &found,
        &["auth".to_owned(), "status".to_owned()],
        &[],
        &dir,
        &cli::variables_to_remove(),
        Duration::from_secs(10),
    )
    .ok()
    .and_then(|output| signed_in_from(&String::from_utf8_lossy(&output.stdout)));
    let account = match signed_in {
        Some((true, Some(plan))) => format!(", signed in with your Claude account ({plan})"),
        Some((true, None)) => ", signed in".to_owned(),
        Some((false, _)) => ", not signed in: run claude in a terminal, then /login".to_owned(),
        None => String::new(),
    };
    format!(
        "Claude Code {} at {}{account}.",
        cli.version,
        found.display()
    )
}

/// Whether `path` runs through `cmd.exe` (an npm install's `claude.cmd`).
fn is_batch(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
    })
}

/// What the stream said: its `result` line, and when the plan's limit resets.
#[derive(Debug, Default)]
pub struct Stream {
    pub result: Option<Value>,
    /// The last rate limit report's reset time, in seconds since 1970.
    pub resets_at: Option<u64>,
    /// The last rate limit report said requests aren't allowed.
    pub limited: bool,
}

/// Reads the stream-json lines Claude Code wrote. Lines that aren't JSON are passed by.
pub fn read_stream(stdout: &[u8]) -> Stream {
    let mut stream = Stream::default();
    for line in String::from_utf8_lossy(stdout).lines() {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match event["type"].as_str() {
            Some("result") => stream.result = Some(event),
            Some("rate_limit_event") => {
                let info = &event["rate_limit_info"];
                stream.resets_at = info["resetsAt"].as_u64().or(stream.resets_at);
                stream.limited = info["status"]
                    .as_str()
                    .is_some_and(|status| status != "allowed" && status != "allowed_warning");
            }
            _ => {}
        }
    }
    stream
}

fn is_signed_out(text: &str) -> bool {
    let text = text.to_lowercase();
    ["/login", "not logged in", "invalid api key", "oauth token"]
        .iter()
        .any(|sign| text.contains(sign))
}

fn is_usage_limit(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "usage limit",
        "limit reached",
        "hit your limit",
        "limit_reached",
        "5-hour limit",
        "weekly limit",
        "daily limit",
    ]
    .iter()
    .any(|sign| text.contains(sign))
}

/// A reset time written into a limit message: `…limit reached|1792429200`.
fn reset_in_text(text: &str) -> Option<u64> {
    let (_, after) = text.rsplit_once('|')?;
    let digits: String = after
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits
        .parse()
        .ok()
        .filter(|seconds| *seconds > 1_000_000_000)
}

/// "at 17:00", or "on 2026-10-21 at 09:00" when it isn't today.
fn when(unix_seconds: u64) -> String {
    let Some((date, time)) = crate::platform::clock::local_at(unix_seconds) else {
        return "later".to_owned();
    };
    let clock = format!("{:02}:{:02}", time.hour, time.minute);
    if date == crate::platform::clock::local_now().0 {
        format!("at {clock}")
    } else {
        format!(
            "on {} at {clock}",
            crate::notebook::dates::format(date, "YYYY-MM-DD")
        )
    }
}

/// What a finished run means: a note, or what the runner should do. `now` is milliseconds since
/// 1970.
pub fn interpret(
    stream: &Stream,
    code: Option<i32>,
    stderr: &str,
    now: u64,
) -> Result<NoteJson, BackendError> {
    let Some(result) = &stream.result else {
        let said = stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim();
        return Err(BackendError::Pause(format!(
            "Claude Code stopped without an answer (exit code {}){}",
            code.map_or_else(|| "none".to_owned(), |code| code.to_string()),
            if said.is_empty() {
                String::new()
            } else {
                format!(": {said}")
            }
        )));
    };
    let text = result["result"].as_str().unwrap_or("");
    if result["is_error"].as_bool() == Some(true) || stream.limited {
        if is_signed_out(text) {
            return Err(BackendError::Pause(
                "Claude Code isn't signed in. Run claude in a terminal, then /login.".to_owned(),
            ));
        }
        if stream.limited || is_usage_limit(text) {
            crate::log::line(&format!("claude code: limit: {text}"));
            let reset = reset_in_text(text)
                .or(stream.resets_at)
                .map(|seconds| seconds * 1000)
                .filter(|at| *at > now)
                .unwrap_or(now + LIMIT_WAIT.as_millis() as u64);
            return Err(BackendError::PauseUntil {
                until: reset,
                reason: format!(
                    "Your Claude plan's usage limit is reached. Notes carry on {}.",
                    when(reset / 1000)
                ),
            });
        }
        let status = result["api_error_status"].as_u64();
        let lower = text.to_lowercase();
        if matches!(status, Some(429 | 500..=599))
            || [
                "overloaded",
                "connection error",
                "timed out",
                "timeout",
                "network",
            ]
            .iter()
            .any(|sign| lower.contains(sign))
        {
            return Err(BackendError::Retry {
                after: None,
                reason: format!("{SERVICE} couldn't reach Claude: {text}"),
            });
        }
        if lower.contains("model") && (lower.contains("not found") || lower.contains("invalid")) {
            return Err(BackendError::Pause(format!(
                "{SERVICE} doesn't know this model ({text}). Choose another in Settings."
            )));
        }
        return Err(BackendError::Fail(format!(
            "{SERVICE}: {}",
            if text.is_empty() {
                "it reported an error"
            } else {
                text
            }
        )));
    }
    if let Some(structured) = result
        .get("structured_output")
        .filter(|value| value.is_object())
    {
        return reply::parse(&structured.to_string()).map_err(BackendError::Invalid);
    }
    if text.trim().is_empty() {
        return Err(BackendError::Invalid(reply::ReplyError(vec![
            "the reply has no text".to_owned(),
        ])));
    }
    reply::parse(text).map_err(BackendError::Invalid)
}

impl Backend for ClaudeCode {
    fn label(&self) -> String {
        format!("cloud/claude-code/{}", self.model)
    }

    fn complete(&self, request: &NoteRequest) -> Result<NoteJson, BackendError> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| BackendError::Fail(format!("{}: {error}", self.dir.display())))?;
        let rules = self.rules(request);
        let rules_file = self.dir.join(RULES_FILE);
        let system = if self.cli.options.system_prompt_file {
            std::fs::write(&rules_file, &rules).map_err(|error| {
                BackendError::Fail(format!("{}: {error}", rules_file.display()))
            })?;
            SystemPrompt::File(&rules_file)
        } else if is_batch(&self.cli.path) {
            // cmd.exe can't be given the rules' lines as one argument.
            return Err(BackendError::Pause(format!(
                "This Claude Code ({}) can't be given FastCapture's rules through {}. Update                  it with claude update, or install the native build.",
                self.cli.version,
                self.cli.path.display()
            )));
        } else {
            SystemPrompt::Text(&rules)
        };
        let schema = schema::schema().to_string();
        let args = cli::args(self.cli.options, &self.model, system, Some(&schema));
        let output = run::run(
            &self.cli.path,
            &args,
            message(request).as_bytes(),
            &self.dir,
            &cli::variables_to_remove(),
            TIMEOUT,
        )
        .map_err(|error| match error {
            RunError::Start(reason) => {
                BackendError::Pause(format!("{SERVICE} couldn't be started: {reason}"))
            }
            RunError::TimedOut => BackendError::Retry {
                after: None,
                reason: format!("{SERVICE} didn't finish within 5 minutes."),
            },
        })?;
        let stream = read_stream(&output.stdout);
        if let Some(result) = &stream.result {
            crate::log::line(&format!(
                "claude code: {} input ({} cached), {} output tokens, {} turns",
                result["usage"]["input_tokens"].as_u64().unwrap_or(0)
                    + result["usage"]["cache_creation_input_tokens"]
                        .as_u64()
                        .unwrap_or(0),
                result["usage"]["cache_read_input_tokens"]
                    .as_u64()
                    .unwrap_or(0),
                result["usage"]["output_tokens"].as_u64().unwrap_or(0),
                result["num_turns"].as_u64().unwrap_or(0)
            ));
        }
        interpret(
            &stream,
            output.code,
            &output.stderr,
            crate::runner::process::now_ms(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::claude_code::cli::tests::{HELP_NOW, Temp};
    use crate::model::images::ModelImage;
    use crate::model::request::CaptureFacts;

    const NOTE: &str = r#"{"title":"Lag","summary":"S","body":"B","key_points":[],"tasks":[],"tags":["kafka"],"links":[],"home":null}"#;

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

    fn result(fields: Value) -> String {
        let mut line = json!({"type": "result", "subtype": "success", "is_error": false, "num_turns": 2,
            "usage": {"input_tokens": 2, "cache_creation_input_tokens": 2137, "cache_read_input_tokens": 0, "output_tokens": 248}});
        for (key, value) in fields.as_object().unwrap() {
            line[key] = value.clone();
        }
        line.to_string()
    }

    fn stream(lines: &[String]) -> Stream {
        read_stream(lines.join("\n").as_bytes())
    }

    #[test]
    fn the_sign_in_state_is_read_from_auth_status() {
        assert_eq!(
            signed_in_from(
                r#"{"loggedIn": true, "authMethod": "claude.ai", "subscriptionType": "max"}"#
            ),
            Some((true, Some("Max".to_owned())))
        );
        assert_eq!(
            signed_in_from(r#"{"loggedIn": false}"#),
            Some((false, None))
        );
        assert_eq!(signed_in_from("error: unknown command 'auth'"), None);
    }

    #[test]
    fn the_status_line_says_what_was_found() {
        let temp = Temp::new("status");
        let fake = temp.script(
            "claude.cmd",
            &[
                r#"if "%~1"=="--version" (echo 2.1.303 ^(Claude Code^)& exit /b 0)"#,
                r#"if "%~1"=="auth" (echo {"loggedIn": true, "subscriptionType": "pro"}& exit /b 0)"#,
            ],
        );
        assert_eq!(
            status(Some(&fake)),
            format!(
                "Claude Code 2.1.303 at {}, signed in with your Claude account (Pro).",
                fake.display()
            )
        );
        assert!(status(Some(&temp.0.join("nope.exe"))).starts_with("Claude Code isn't at"));
    }

    #[test]
    fn the_message_carries_the_image_then_the_text() {
        let line = message(&request());
        assert!(line.ends_with('\n') && !line.trim_end().contains('\n'));
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["type"], "user");
        let content = value["message"]["content"].as_array().unwrap();
        assert_eq!(content[0]["source"]["data"], "cG5n");
        assert_eq!(content[0]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["text"], "<your_note>\nlag\n</your_note>");
    }

    #[test]
    fn a_note_comes_from_the_structured_output_or_the_result_text() {
        let structured: Value = serde_json::from_str(NOTE).unwrap();
        let from_schema = stream(&[
            r#"{"type":"system","subtype":"init","tools":["StructuredOutput"]}"#.to_owned(),
            "not json".to_owned(),
            result(json!({"result": "ignored", "structured_output": structured})),
        ]);
        assert_eq!(
            interpret(&from_schema, Some(0), "", 0).unwrap().title,
            "Lag"
        );
        let from_text = stream(&[result(json!({"result": format!("Here it is: {NOTE}")}))]);
        assert_eq!(
            interpret(&from_text, Some(0), "", 0).unwrap().tags,
            ["kafka"]
        );
        let unusable = stream(&[result(json!({"result": "Sorry, no."}))]);
        assert!(matches!(
            interpret(&unusable, Some(0), "", 0),
            Err(BackendError::Invalid(_))
        ));
    }

    #[test]
    fn signed_out_pauses_and_says_how_to_sign_in() {
        // As 2.0.42 answered when it wasn't signed in: is_error, though subtype is success.
        let signed_out = stream(&[result(
            json!({"is_error": true, "result": "Invalid API key · Please run /login"}),
        )]);
        assert_eq!(
            interpret(&signed_out, Some(1), "", 0).unwrap_err(),
            BackendError::Pause(
                "Claude Code isn't signed in. Run claude in a terminal, then /login.".to_owned()
            )
        );
        let not_logged_in = stream(&[result(
            json!({"is_error": true, "result": "Not logged in · Please run /login"}),
        )]);
        assert!(matches!(
            interpret(&not_logged_in, Some(1), "", 0),
            Err(BackendError::Pause(_))
        ));
    }

    #[test]
    fn a_usage_limit_pauses_until_it_resets_or_for_half_an_hour() {
        let now = 1_792_400_000_000;
        let with_time = stream(&[result(
            json!({"is_error": true, "result": "Claude AI usage limit reached|1792429200"}),
        )]);
        assert!(matches!(
            interpret(&with_time, Some(1), "", now),
            Err(BackendError::PauseUntil { until: 1_792_429_200_000, ref reason })
                if reason.starts_with("Your Claude plan's usage limit is reached. Notes carry on ")
        ));
        let from_event = stream(&[
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1792429800,"rateLimitType":"five_hour"}}"#.to_owned(),
            result(json!({"is_error": true, "result": "5-hour limit reached ∙ resets 5pm"})),
        ]);
        assert!(matches!(
            interpret(&from_event, Some(1), "", now),
            Err(BackendError::PauseUntil {
                until: 1_792_429_800_000,
                ..
            })
        ));
        let no_time = stream(&[result(
            json!({"is_error": true, "result": "You've hit your limit"}),
        )]);
        assert!(matches!(
            interpret(&no_time, Some(1), "", now),
            Err(BackendError::PauseUntil { until, .. }) if until == now + 30 * 60 * 1000
        ));
        let allowed = stream(&[
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1792429200}}"#.to_owned(),
            result(json!({"result": NOTE})),
        ]);
        assert!(
            interpret(&allowed, Some(0), "", now).is_ok(),
            "a report that it is allowed isn't a limit"
        );
    }

    #[test]
    fn other_errors_retry_pause_or_fail() {
        let overloaded = stream(&[result(
            json!({"is_error": true, "api_error_status": 529, "result": "API Error: Overloaded"}),
        )]);
        assert!(matches!(
            interpret(&overloaded, Some(1), "", 0),
            Err(BackendError::Retry { .. })
        ));
        let model = stream(&[result(
            json!({"is_error": true, "result": "The model claude-nope was not found"}),
        )]);
        assert!(matches!(
            interpret(&model, Some(1), "", 0),
            Err(BackendError::Pause(m)) if m.contains("doesn't know this model")
        ));
        let other = stream(&[result(
            json!({"is_error": true, "subtype": "error_during_execution", "result": "Something else"}),
        )]);
        assert_eq!(
            interpret(&other, Some(1), "", 0).unwrap_err(),
            BackendError::Fail("Claude Code: Something else".to_owned())
        );
        assert_eq!(
            interpret(&Stream::default(), Some(1), "\nerror: unknown option '--json-schema'\n", 0).unwrap_err(),
            BackendError::Pause(
                "Claude Code stopped without an answer (exit code 1): error: unknown option '--json-schema'".to_owned()
            )
        );
    }

    /// A fake `claude.cmd` that writes the stdin it got and the arguments to files, and prints
    /// `reply`'s lines.
    fn fake(temp: &Temp, reply: &[String]) -> Cli {
        let mut lines = vec![
            format!(r#"findstr "^" > "{}""#, temp.0.join("stdin.txt").display()),
            format!(r#"echo %* > "{}""#, temp.0.join("args.txt").display()),
        ];
        let replies = temp.0.join("reply.jsonl");
        std::fs::write(&replies, reply.join("\n")).unwrap();
        lines.push(format!(r#"type "{}""#, replies.display()));
        Cli {
            path: temp.script(
                "claude.cmd",
                &lines.iter().map(String::as_str).collect::<Vec<_>>(),
            ),
            version: "2.1.303".into(),
            options: cli::options_from_help(HELP_NOW),
        }
    }

    #[test]
    fn a_fake_claude_code_gets_the_request_and_its_answer_becomes_the_note() {
        let temp = Temp::new("backend");
        let structured: Value = serde_json::from_str(NOTE).unwrap();
        let cli = fake(&temp, &[result(json!({"structured_output": structured}))]);
        let backend = ClaudeCode::with_cli(cli, "haiku", temp.0.join("run"));
        assert_eq!(backend.label(), "cloud/claude-code/haiku");
        assert_eq!(backend.complete(&request()).unwrap().title, "Lag");
        assert_eq!(backend.take_usage(), None, "a plan's usage isn't counted");

        let stdin = std::fs::read_to_string(temp.0.join("stdin.txt")).unwrap();
        let sent: Value = serde_json::from_str(stdin.trim()).unwrap();
        assert_eq!(sent["message"]["content"][0]["type"], "image");
        let args = std::fs::read_to_string(temp.0.join("args.txt")).unwrap();
        assert!(args.contains(r#"--tools """#), "{args}");
        assert!(args.contains("--model haiku"), "{args}");
        assert!(args.contains("--system-prompt-file"), "{args}");
        assert!(args.contains("--json-schema"), "{args}");
        let rules = std::fs::read_to_string(temp.0.join(r"run\rules.md")).unwrap();
        assert_eq!(
            rules,
            crate::model::prompt::RULES,
            "the schema goes as --json-schema"
        );
    }

    #[test]
    fn without_json_schema_the_schema_goes_in_the_rules() {
        let temp = Temp::new("backend-old");
        let mut cli = fake(&temp, &[result(json!({"result": NOTE}))]);
        cli.options.json_schema = false;
        let backend = ClaudeCode::with_cli(cli, "sonnet", temp.0.join("run"));
        assert_eq!(backend.complete(&request()).unwrap().title, "Lag");
        let args = std::fs::read_to_string(temp.0.join("args.txt")).unwrap();
        assert!(!args.contains("--json-schema"), "{args}");
        let rules = std::fs::read_to_string(temp.0.join(r"run\rules.md")).unwrap();
        assert!(rules.contains("The JSON schema the answer must match"));
    }

    #[test]
    fn an_old_npm_install_without_a_rules_file_is_named_not_run() {
        let temp = Temp::new("backend-npm");
        let mut cli = fake(&temp, &[result(json!({"result": NOTE}))]);
        cli.options = cli::Options::default();
        cli.version = "2.0.42".into();
        let backend = ClaudeCode::with_cli(cli, "sonnet", temp.0.join("run"));
        assert!(matches!(
            backend.complete(&request()),
            Err(BackendError::Pause(m)) if m.starts_with("This Claude Code (2.0.42) can't be given")
        ));
        assert!(!temp.0.join("args.txt").exists(), "it wasn't started");
    }
}
