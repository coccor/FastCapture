//! One job, start to finish: fetch its links' pages, build the request, ask the backend (once
//! more if the reply isn't a usable note), check the reply, write or replace the note, and
//! record what happened. A finding an agent wrote is placed instead: its request asks only for
//! links and a home, which go into the note as it is.

use super::backoff;
use crate::index::service::{FIRST_BUILD_WAIT, Indexes};
use crate::model::request;
use crate::model::{Backend, BackendError};
use crate::note::reply::{self, Allowed};
use crate::note::{NoteJson, place, render, write};
use crate::notebook::Conventions;
use crate::queue::{Job, State, Store};
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The note is written; the job is done.
    Written(write::Written),
    /// The job waits until `at` (milliseconds since 1970).
    Retry { at: u64, reason: String },
    /// The whole queue should stop until the user fixes something; the job stays queued.
    Pause(String),
    /// The whole queue should stop until `until` (milliseconds since 1970); the job stays
    /// queued.
    PauseUntil { until: u64, reason: String },
    /// The job failed for good.
    Failed(String),
}

/// Runs `job` with `backend` at `now` (milliseconds since 1970) on `today` (`YYYY-MM-DD`), and
/// saves the job's new state in `store`. No candidate notes are offered.
pub fn process(store: &Store, job: Job, backend: &dyn Backend, now: u64, today: &str) -> Outcome {
    process_with(store, job, backend, now, today, None)
}

/// `process`, with candidate notes from `indexes` when given.
pub fn process_with(
    store: &Store,
    mut job: Job,
    backend: &dyn Backend,
    now: u64,
    today: &str,
    indexes: Option<&Indexes>,
) -> Outcome {
    job.state = State::Working;
    job.attempts += 1;
    save(store, &job);

    let outcome = run(&job, backend, today, indexes);
    match &outcome {
        Ok(written) => {
            job.state = State::Done;
            job.note = Some(written.clone());
            job.regenerate = false;
            job.retry_at = None;
            job.last_error = None;
        }
        Err(BackendError::Retry { after, reason }) => {
            let wait = backoff::delay(job.attempts, *after);
            job.state = State::Queued;
            job.retry_at = Some(now + wait.as_millis() as u64);
            job.last_error = Some(reason.clone());
        }
        Err(BackendError::Pause(reason) | BackendError::PauseUntil { reason, .. }) => {
            // Not the job's fault: it keeps its place, and its try doesn't count.
            job.state = State::Queued;
            job.attempts = job.attempts.saturating_sub(1);
            job.last_error = Some(reason.clone());
        }
        Err(error) => {
            job.state = State::Failed;
            job.last_error = Some(error.to_string());
        }
    }
    save(store, &job);
    match outcome {
        Ok(written) => Outcome::Written(written),
        Err(BackendError::Retry { reason, .. }) => Outcome::Retry {
            at: job.retry_at.unwrap_or(now),
            reason,
        },
        Err(BackendError::Pause(reason)) => Outcome::Pause(reason),
        Err(BackendError::PauseUntil { until, reason }) => Outcome::PauseUntil { until, reason },
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

fn save(store: &Store, job: &Job) {
    if let Err(error) = store.put(job) {
        crate::log::line(&format!("runner: {error}"));
    }
}

fn run(
    job: &Job,
    backend: &dyn Backend,
    today: &str,
    indexes: Option<&Indexes>,
) -> Result<write::Written, BackendError> {
    if job.finding {
        return place_finding(job, backend, today, indexes);
    }
    let (date, time) = crate::platform::clock::local_now();
    super::pages::fetch_pages(job, &crate::capture::raw::captured_stamp(date, time))?;
    // Without the index in time, the note is written without candidates.
    let snapshot = indexes.and_then(|indexes| indexes.snapshot(&job.notebook, FIRST_BUILD_WAIT));
    let request =
        request::build_with(job, today, snapshot.as_deref()).map_err(BackendError::Fail)?;
    let note = ask(backend, &request)?;
    let facts = &request.capture;
    let note = reply::check(
        note,
        &facts.fallback_title,
        &Allowed {
            notes: &facts.candidates,
            home_notes: &facts.home_notes,
            home_folders: &facts.home_folders,
        },
    );
    let conventions = Conventions::load(&facts.notebook);
    let path = match (&job.note, job.regenerate) {
        (Some(old), true) => old.path.clone(),
        _ => write::note_path(
            &facts.notebook,
            conventions.capture_folder(),
            &render::file_stem(&note.title),
        ),
    };
    let text = render::render(&render::NoteFile {
        note: &note,
        captured: &facts.captured,
        capture_id: &facts.capture_id,
        raw: &facts.raw,
        kinds: &facts.kinds,
        model: &backend.label(),
        attachments: &facts.attachments,
        links: &facts.links,
        truncated: &facts.truncated,
        path: &path,
    });
    match (&job.note, job.regenerate) {
        (Some(old), true) => {
            write::replace(&facts.notebook, old, &text).map_err(|error| match error {
                write::ReplaceError::Edited => BackendError::Fail(
                    "The note was edited since FastCapture wrote it, so it wasn't replaced."
                        .to_owned(),
                ),
                write::ReplaceError::Failed(message) => BackendError::Fail(message),
            })
        }
        _ => write::write_new(&facts.notebook, &path, &text).map_err(BackendError::Fail),
    }
}

/// Asks `backend` for `request`, once more when the reply isn't usable.
fn ask(backend: &dyn Backend, request: &request::NoteRequest) -> Result<NoteJson, BackendError> {
    match backend.complete(request) {
        Err(BackendError::Invalid(problems)) => {
            crate::log::line(&format!("runner: asking again ({problems})"));
            backend.complete(&request.corrected(&problems.to_string()))
        }
        other => other,
    }
}

/// How long a finding that changed while being placed waits before it is tried again.
const CHANGED_WAIT: Duration = Duration::from_secs(5);

/// Places `job`'s finding: its links and home, from candidates in `indexes` when given.
fn place_finding(
    job: &Job,
    backend: &dyn Backend,
    today: &str,
    indexes: Option<&Indexes>,
) -> Result<write::Written, BackendError> {
    let snapshot = indexes.and_then(|indexes| indexes.snapshot(&job.notebook, FIRST_BUILD_WAIT));
    let request =
        request::build_placing(job, today, snapshot.as_deref()).map_err(BackendError::Fail)?;
    let note = ask(backend, &request)?;
    let facts = &request.capture;
    let note = reply::check(
        note,
        &facts.fallback_title,
        &Allowed {
            notes: &facts.candidates,
            home_notes: &facts.home_notes,
            home_folders: &facts.home_folders,
        },
    );
    place::place(&facts.notebook, &facts.raw, &note).map_err(|error| match error {
        write::ReplaceError::Edited => BackendError::Retry {
            after: Some(CHANGED_WAIT),
            reason: "The note changed while it was being placed.".to_owned(),
        },
        write::ReplaceError::Failed(message) => BackendError::Fail(message),
    })
}

/// Milliseconds since 1970, now.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// How long until `at` from `now`, at least zero.
pub fn until(at: u64, now: u64) -> Duration {
    Duration::from_millis(at.saturating_sub(now))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::capture::Capture;
    use crate::capture::submit::submit;
    use crate::model::request::NoteRequest;
    use crate::note::NoteJson;
    use crate::note::reply::ReplyError;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Answers with each scripted result in turn, and records the requests' user turns.
    pub(crate) struct Scripted {
        pub answers: Mutex<Vec<Result<NoteJson, BackendError>>>,
        pub asked: Mutex<Vec<String>>,
    }

    impl Scripted {
        pub(crate) fn new(answers: Vec<Result<NoteJson, BackendError>>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().rev().collect()),
                asked: Mutex::new(Vec::new()),
            }
        }
    }

    impl Backend for Scripted {
        fn label(&self) -> String {
            "local/test-model".to_owned()
        }

        fn complete(&self, request: &NoteRequest) -> Result<NoteJson, BackendError> {
            self.asked.lock().unwrap().push(request.user_text.clone());
            self.answers
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| Err(BackendError::Fail("script ran out".to_owned())))
        }
    }

    pub(crate) fn note(title: &str) -> NoteJson {
        NoteJson {
            title: title.to_owned(),
            summary: "A summary.".to_owned(),
            body: "The body.".to_owned(),
            ..NoteJson::default()
        }
    }

    pub(crate) struct Temp(pub PathBuf);

    impl Temp {
        pub(crate) fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("fastcapture-runner-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("Notes")).unwrap();
            Self(dir)
        }

        pub(crate) fn notebook(&self) -> PathBuf {
            self.0.join("Notes")
        }

        pub(crate) fn store(&self) -> Store {
            Store::new(self.0.join("queue"))
        }

        /// Captures `text` and returns its job.
        pub(crate) fn capture(&self, text: &str) -> Job {
            submit(
                &self.notebook(),
                &Capture {
                    text: text.to_owned(),
                    ..Capture::default()
                },
                Some(&self.0.join("queue")),
                false,
            )
            .unwrap();
            self.store()
                .list()
                .into_iter()
                .find(|job| job.state == State::Queued && job.note.is_none())
                .unwrap()
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_good_reply_becomes_a_note_and_the_job_is_done() {
        let temp = Temp::new("good");
        let job = temp.capture("lag spikes during ledger batch??");
        let backend = Scripted::new(vec![Ok(note("Consumer lag spikes"))]);
        let outcome = process(&temp.store(), job.clone(), &backend, 1000, "2026-10-19");
        let Outcome::Written(written) = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(written.path, PathBuf::from(r"inbox\Consumer lag spikes.md"));
        let text = std::fs::read_to_string(temp.notebook().join(&written.path)).unwrap();
        assert!(
            text.contains("model: \"local/test-model\"")
                && text.contains("# Consumer lag spikes\n")
        );
        assert!(text.contains(&format!("capture_id: {}", job.capture_id)));
        let saved = temp.store().get(&job.capture_id).unwrap();
        assert_eq!(
            (saved.state, saved.attempts, saved.note),
            (State::Done, 1, Some(written))
        );
    }

    #[test]
    fn an_unusable_reply_is_asked_for_once_more() {
        let temp = Temp::new("invalid");
        let job = temp.capture("x");
        let problems = ReplyError(vec!["\"title\" is missing".to_owned()]);
        let backend = Scripted::new(vec![
            Err(BackendError::Invalid(problems.clone())),
            Ok(note("Fixed")),
        ]);
        assert!(matches!(
            process(&temp.store(), job, &backend, 0, "2026-10-19"),
            Outcome::Written(_)
        ));
        let asked = backend.asked.lock().unwrap();
        assert_eq!(asked.len(), 2);
        assert!(asked[1].ends_with("Your previous reply couldn't be used: \"title\" is missing. Answer again with the JSON object only."));

        let job = temp.capture("y");
        let backend = Scripted::new(vec![
            Err(BackendError::Invalid(problems.clone())),
            Err(BackendError::Invalid(problems)),
        ]);
        assert!(matches!(
            process(&temp.store(), job.clone(), &backend, 0, "2026-10-19"),
            Outcome::Failed(_)
        ));
        assert_eq!(
            temp.store().get(&job.capture_id).unwrap().state,
            State::Failed
        );
    }

    #[test]
    fn retries_wait_pauses_keep_the_place_and_failures_stop() {
        let temp = Temp::new("errors");
        let job = temp.capture("x");
        let backend = Scripted::new(vec![
            Err(BackendError::Retry {
                after: None,
                reason: "offline".into(),
            }),
            Err(BackendError::Retry {
                after: Some(Duration::from_secs(7)),
                reason: "busy".into(),
            }),
            Err(BackendError::Pause("bad key".into())),
            Err(BackendError::Fail("refused".into())),
        ]);
        let store = temp.store();
        assert_eq!(
            process(&store, job.clone(), &backend, 1_000, "d"),
            Outcome::Retry {
                at: 31_000,
                reason: "offline".into()
            }
        );
        let job = store.get(&job.capture_id).unwrap();
        assert!(!job.is_due(30_999) && job.is_due(31_000));
        assert_eq!(
            process(&store, job.clone(), &backend, 40_000, "d"),
            Outcome::Retry {
                at: 47_000,
                reason: "busy".into()
            }
        );
        let job = store.get(&job.capture_id).unwrap();
        assert_eq!(
            process(&store, job.clone(), &backend, 50_000, "d"),
            Outcome::Pause("bad key".into())
        );
        let job = store.get(&job.capture_id).unwrap();
        assert_eq!(
            (job.state, job.attempts),
            (State::Queued, 2),
            "a pause doesn't count"
        );
        assert_eq!(
            process(&store, job.clone(), &backend, 60_000, "d"),
            Outcome::Failed("refused".into())
        );
        assert_eq!(
            store.get(&job.capture_id).unwrap().last_error.as_deref(),
            Some("refused")
        );
    }

    #[test]
    fn regenerate_replaces_the_note_unless_it_was_edited() {
        let temp = Temp::new("regenerate");
        let job = temp.capture("x");
        let store = temp.store();
        let backend = Scripted::new(vec![
            Ok(note("First")),
            Ok(note("Second")),
            Ok(note("Third")),
        ]);
        let Outcome::Written(first) = process(&store, job.clone(), &backend, 0, "d") else {
            panic!()
        };
        let mut again = store.get(&job.capture_id).unwrap();
        again.regenerate = true;
        again.state = State::Queued;
        let Outcome::Written(second) = process(&store, again, &backend, 0, "d") else {
            panic!()
        };
        assert_eq!(second.path, first.path, "same file, new text");
        let text = std::fs::read_to_string(temp.notebook().join(&second.path)).unwrap();
        assert!(text.contains("# Second\n"));

        std::fs::write(temp.notebook().join(&second.path), "my edits").unwrap();
        let mut third = store.get(&job.capture_id).unwrap();
        third.regenerate = true;
        third.state = State::Queued;
        let outcome = process(&store, third, &backend, 0, "d");
        assert!(
            matches!(&outcome, Outcome::Failed(m) if m.contains("edited")),
            "{outcome:?}"
        );
        assert_eq!(
            std::fs::read_to_string(temp.notebook().join(&second.path)).unwrap(),
            "my edits"
        );
    }

    #[test]
    fn a_missing_raw_source_fails_the_job() {
        let temp = Temp::new("missing");
        let job = temp.capture("x");
        std::fs::remove_file(temp.notebook().join(&job.raw)).unwrap();
        let backend = Scripted::new(Vec::new());
        assert!(
            matches!(process(&temp.store(), job, &backend, 0, "d"), Outcome::Failed(m) if m.contains("could not read"))
        );
        assert!(backend.asked.lock().unwrap().is_empty());
    }
}
