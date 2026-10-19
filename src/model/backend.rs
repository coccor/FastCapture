//! What every model backend does: one request in, one checked-or-not note out, and errors that
//! tell the runner whether to wait, pause the queue, give up on the job, or ask again.

use super::request::NoteRequest;
use crate::net::http::{HttpError, Response};
use crate::note::NoteJson;
use crate::note::reply::ReplyError;
use std::sync::Mutex;
use std::time::Duration;

/// The tokens requests used, as the API counted them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Usage {
    /// Input not read from the cache.
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl Usage {
    /// Every token, cached ones included.
    pub fn total(&self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.output
    }

    pub fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.output += other.output;
    }
}

/// What a backend's requests used since it was last asked.
#[derive(Debug, Default)]
pub struct Meter(Mutex<Option<Usage>>);

impl Meter {
    pub fn add(&self, usage: Usage) {
        if let Ok(mut meter) = self.0.lock() {
            meter.get_or_insert_with(Usage::default).add(usage);
        }
    }

    pub fn take(&self) -> Option<Usage> {
        self.0.lock().ok().and_then(|mut meter| meter.take())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BackendError {
    /// Try this job again later (offline, overloaded, rate limited). `after` is the server's
    /// own wait, when it gave one.
    Retry {
        after: Option<Duration>,
        reason: String,
    },
    /// Stop the whole queue until the user fixes something (a refused key, billing).
    Pause(String),
    /// This job can't be done as it is (a request the server refuses, a declined capture).
    Fail(String),
    /// The reply came but isn't a usable note; the runner asks once more.
    Invalid(ReplyError),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retry { reason, .. } | Self::Pause(reason) | Self::Fail(reason) => {
                f.write_str(reason)
            }
            Self::Invalid(problems) => write!(f, "the reply wasn't a usable note: {problems}"),
        }
    }
}

pub trait Backend: Send {
    /// How notes written by this backend name it: `cloud/claude-sonnet-5-5`, `local/llama3.2`.
    fn label(&self) -> String;
    /// Asks for one note. The note is parsed, not yet checked (`note::reply::check`).
    fn complete(&self, request: &NoteRequest) -> Result<NoteJson, BackendError>;
    /// The tokens this backend's requests used since it was last asked, when the API said.
    fn take_usage(&self) -> Option<Usage> {
        None
    }
}

/// A transport failure as a backend error: offline and timeouts retry, the rest fail.
pub fn from_http(error: HttpError, service: &str) -> BackendError {
    match error {
        HttpError::Transient(reason) => BackendError::Retry {
            after: None,
            reason: format!("{service} can't be reached: {reason}"),
        },
        HttpError::Permanent(reason) => BackendError::Fail(format!("{service}: {reason}")),
    }
}

/// The error message in a JSON error body (`{"error": {"message": …}}`, or `{"error": "…"}`),
/// else the body's start.
pub fn error_message(response: &Response) -> String {
    let body = String::from_utf8_lossy(&response.body);
    let parsed: Option<serde_json::Value> = serde_json::from_str(&body).ok();
    let message = parsed.as_ref().and_then(|value| {
        let error = value.get("error")?;
        error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .or_else(|| error.as_str())
            .map(str::to_owned)
    });
    message.unwrap_or_else(|| body.chars().take(300).collect::<String>().trim().to_owned())
}

/// A non-success status as a backend error, the same way for every HTTP backend. `service`
/// names it in messages ("Claude", "the local model server").
pub fn from_status(response: &Response, service: &str) -> BackendError {
    let message = error_message(response);
    match response.status {
        401 | 403 => BackendError::Pause(format!(
            "{service} refused the API key ({message}). Check it in Settings."
        )),
        402 => BackendError::Pause(format!("{service} reports a billing problem: {message}")),
        404 => BackendError::Fail(format!(
            "{service} doesn't know this model or address ({message}). Choose another in Settings."
        )),
        429 => BackendError::Retry {
            after: response
                .header("retry-after")
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map(Duration::from_secs),
            reason: format!("{service} is rate limiting requests: {message}"),
        },
        500..=599 => BackendError::Retry {
            after: response
                .header("retry-after")
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map(Duration::from_secs),
            reason: format!("{service} is unavailable ({}): {message}", response.status),
        },
        status => BackendError::Fail(format!(
            "{service} refused the request ({status}): {message}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &str, headers: &[(&str, &str)]) -> Response {
        Response {
            status,
            url: String::new(),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn statuses_map_to_what_the_runner_does() {
        let claude =
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert!(
            matches!(from_status(&response(401, claude, &[]), "Claude"), BackendError::Pause(m) if m.contains("API key"))
        );
        assert!(matches!(
            from_status(&response(402, claude, &[]), "Claude"),
            BackendError::Pause(_)
        ));
        assert!(matches!(
            from_status(&response(404, claude, &[]), "Claude"),
            BackendError::Fail(_)
        ));
        assert!(
            matches!(from_status(&response(400, claude, &[]), "Claude"), BackendError::Fail(m) if m.ends_with("Overloaded"))
        );
        assert_eq!(
            from_status(&response(429, claude, &[("Retry-After", "7")]), "Claude"),
            BackendError::Retry {
                after: Some(Duration::from_secs(7)),
                reason: "Claude is rate limiting requests: Overloaded".to_owned()
            }
        );
        assert!(matches!(
            from_status(&response(529, claude, &[]), "Claude"),
            BackendError::Retry { after: None, .. }
        ));
    }

    #[test]
    fn a_meter_adds_requests_up_until_it_is_read() {
        let meter = Meter::default();
        assert_eq!(meter.take(), None);
        meter.add(Usage {
            input: 10,
            cache_read: 5,
            cache_write: 0,
            output: 3,
        });
        meter.add(Usage {
            input: 1,
            cache_read: 0,
            cache_write: 2,
            output: 4,
        });
        let usage = meter.take().unwrap();
        assert_eq!(
            usage,
            Usage {
                input: 11,
                cache_read: 5,
                cache_write: 2,
                output: 7
            }
        );
        assert_eq!(usage.total(), 25);
        assert_eq!(meter.take(), None, "taken");
    }

    #[test]
    fn error_messages_come_from_either_json_shape_or_the_body() {
        assert_eq!(
            error_message(&response(400, r#"{"error":"model not found"}"#, &[])),
            "model not found"
        );
        assert_eq!(
            error_message(&response(502, "<html>Bad gateway</html>", &[])),
            "<html>Bad gateway</html>"
        );
    }
}
