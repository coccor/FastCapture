//! A job: one raw source waiting to become a note, with its state, the note it became, and when
//! to try again. Stored as JSON, converted by hand so the queue needs no derive macros.
//! Version 1 files (before notes) still read.

use crate::note::write::{Stamp, Written};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

const VERSION: u64 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Queued,
    Working,
    Done,
    Failed,
    /// You discarded its note: the raw source stays, and it isn't processed again until
    /// you regenerate it.
    Discarded,
}

impl State {
    pub fn key(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Working => "working",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Discarded => "discarded",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        [
            Self::Queued,
            Self::Working,
            Self::Done,
            Self::Failed,
            Self::Discarded,
        ]
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
    /// The note written for it, as FastCapture last wrote it.
    pub note: Option<Written>,
    /// Write the note again, replacing the one in `note`.
    pub regenerate: bool,
    /// Not before this time (milliseconds since 1970), after a failure that may pass.
    pub retry_at: Option<u64>,
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
            note: None,
            regenerate: false,
            retry_at: None,
        }
    }

    /// Whether the worker may take it at `now` (milliseconds since 1970).
    pub fn is_due(&self, now: u64) -> bool {
        self.state == State::Queued && self.retry_at.is_none_or(|at| at <= now)
    }

    pub fn to_json(&self) -> String {
        let note = self.note.as_ref().map(|note| {
            json!({
                "path": note.path.to_string_lossy(),
                "len": note.stamp.len,
                "modified_ms": note.stamp.modified_ms,
            })
        });
        let value = json!({
            "version": VERSION,
            "capture_id": self.capture_id,
            "notebook": self.notebook.to_string_lossy(),
            "raw": self.raw.to_string_lossy(),
            "state": self.state.key(),
            "attempts": self.attempts,
            "last_error": self.last_error,
            "created": self.created,
            "note": note,
            "regenerate": self.regenerate,
            "retry_at": self.retry_at,
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
        if !matches!(
            object.get("version").and_then(Value::as_u64),
            Some(1..=VERSION)
        ) {
            return Err("unknown version".to_owned());
        }
        let state = string("state")?;
        let note = object.get("note").and_then(|note| {
            Some(Written {
                path: PathBuf::from(note.get("path")?.as_str()?),
                stamp: Stamp {
                    len: note.get("len")?.as_u64()?,
                    modified_ms: note.get("modified_ms")?.as_u64()?,
                },
            })
        });
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
            note,
            regenerate: object
                .get("regenerate")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            retry_at: object.get("retry_at").and_then(Value::as_u64),
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
        job.state = State::Done;
        job.attempts = 3;
        job.last_error = Some("model \"x\" not found".to_owned());
        job.note = Some(Written {
            path: PathBuf::from(r"inbox\Lag.md"),
            stamp: Stamp {
                len: 812,
                modified_ms: 1_792_281_600_123,
            },
        });
        job.regenerate = true;
        job.retry_at = Some(1_792_281_630_000);
        assert_eq!(Job::from_json(&job.to_json()), Ok(job));
    }

    #[test]
    fn version_1_files_still_read() {
        let job = Job::from_json(
            r#"{"version": 1, "capture_id": "A", "notebook": "n", "raw": "r", "state": "queued", "attempts": 0, "last_error": null, "created": "c"}"#,
        )
        .unwrap();
        assert_eq!(
            (job.note, job.regenerate, job.retry_at),
            (None, false, None)
        );
    }

    #[test]
    fn jobs_are_due_when_queued_and_past_their_retry_time() {
        let mut job = Job::queued("A", PathBuf::new(), PathBuf::new(), String::new());
        assert!(job.is_due(0));
        job.retry_at = Some(100);
        assert!(!job.is_due(99));
        assert!(job.is_due(100));
        job.state = State::Failed;
        assert!(!job.is_due(1000));
    }

    #[test]
    fn broken_jobs_say_what_is_wrong() {
        assert!(Job::from_json("{").is_err());
        assert_eq!(Job::from_json("[]"), Err("not a JSON object".to_owned()));
        assert_eq!(
            Job::from_json(r#"{"version": 3}"#),
            Err("unknown version".to_owned())
        );
        assert_eq!(
            Job::from_json(
                r#"{"version": 2, "capture_id": "x", "notebook": "n", "raw": "r", "state": "lost", "created": "c"}"#
            ),
            Err("unknown state \"lost\"".to_owned())
        );
    }
}
