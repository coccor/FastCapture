//! A job: one raw source waiting to become a note, with its state. Stored as JSON,
//! converted by hand so the queue needs no derive macros.

use serde_json::{Map, Value, json};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Queued,
    Working,
    Done,
    Failed,
}

impl State {
    pub fn key(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Working => "working",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        [Self::Queued, Self::Working, Self::Done, Self::Failed]
            .into_iter()
            .find(|state| state.key() == key)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Job {
    pub capture_id: String,
    /// The notebook's folder.
    pub notebook: PathBuf,
    /// The raw source, relative to the notebook.
    pub raw: PathBuf,
    pub state: State,
    pub attempts: u32,
    pub last_error: Option<String>,
    /// When the capture was made, as in the raw source's `captured:` (`YYYY-MM-DDTHH:MM`).
    pub created: String,
}

impl Job {
    /// A new queued job.
    pub fn queued(capture_id: &str, notebook: PathBuf, raw: PathBuf, created: String) -> Self {
        Self {
            capture_id: capture_id.to_owned(),
            notebook,
            raw,
            state: State::Queued,
            attempts: 0,
            last_error: None,
            created,
        }
    }

    pub fn to_json(&self) -> String {
        let value = json!({
            "version": 1,
            "capture_id": self.capture_id,
            "notebook": self.notebook.to_string_lossy(),
            "raw": self.raw.to_string_lossy(),
            "state": self.state.key(),
            "attempts": self.attempts,
            "last_error": self.last_error,
            "created": self.created,
        });
        serde_json::to_string_pretty(&value).expect("a JSON value always serializes")
    }

    /// The job in `text`, or why it isn't one.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
        let object = value.as_object().ok_or("not a JSON object")?;
        let string = |key: &str| -> Result<String, String> {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("missing \"{key}\""))
        };
        if object.get("version").and_then(Value::as_u64) != Some(1) {
            return Err("unknown version".to_owned());
        }
        let state = string("state")?;
        Ok(Self {
            capture_id: string("capture_id")?,
            notebook: PathBuf::from(string("notebook")?),
            raw: PathBuf::from(string("raw")?),
            state: State::from_key(&state).ok_or_else(|| format!("unknown state \"{state}\""))?,
            attempts: object
                .get("attempts")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            last_error: optional(object, "last_error"),
            created: string("created")?,
        })
    }
}

fn optional(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key).and_then(Value::as_str).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_round_trips_through_json() {
        let mut job = Job::queued(
            "01JAB3K9X7Q2M8ZZZZZZZZZZZZ",
            PathBuf::from(r"C:\Notes"),
            PathBuf::from(r"raw\2026-10-19 1030 lag.md"),
            "2026-10-19T10:30".to_owned(),
        );
        assert_eq!(Job::from_json(&job.to_json()), Ok(job.clone()));
        job.state = State::Failed;
        job.attempts = 3;
        job.last_error = Some("model \"x\" not found".to_owned());
        assert_eq!(Job::from_json(&job.to_json()), Ok(job));
    }

    #[test]
    fn broken_jobs_say_what_is_wrong() {
        assert!(Job::from_json("{").is_err());
        assert_eq!(Job::from_json("[]"), Err("not a JSON object".to_owned()));
        assert_eq!(
            Job::from_json(r#"{"version": 2}"#),
            Err("unknown version".to_owned())
        );
        assert_eq!(
            Job::from_json(
                r#"{"version": 1, "capture_id": "x", "notebook": "n", "raw": "r", "state": "lost", "created": "c"}"#
            ),
            Err("unknown state \"lost\"".to_owned())
        );
    }
}
