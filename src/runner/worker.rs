//! The worker: one thread that takes queued jobs oldest first and turns them into notes. It
//! sleeps until a capture arrives (`Waker::wake`), a retry falls due, or a minute passes; it
//! stops taking jobs while paused, by you or by an error only you can fix; and it builds its
//! backend again when the settings change.

use super::process::{self, Outcome};
use crate::index::service::Indexes;
use crate::model::Backend;
use crate::queue::{State, Store};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The longest the worker sleeps without a reason, in case a job arrived some other way.
const IDLE_CHECK: Duration = Duration::from_secs(60);

/// Builds the backend the settings choose, or says what's missing.
pub trait Factory: Send {
    fn backend(&self) -> Result<Box<dyn Backend>, String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Status {
    /// Nothing to do.
    Idle,
    Working,
    /// Jobs are waiting for a retry, the soonest at `until` (milliseconds since 1970).
    Waiting {
        reason: String,
        until: u64,
    },
    /// Paused by you (`None`) or by an error only you can fix.
    Paused(Option<String>),
    /// No backend is set up yet.
    Unready(String),
}

#[derive(Default)]
struct Signals {
    woken: bool,
    rebuild: bool,
    stop: bool,
    paused_by_user: bool,
    /// An error paused the queue; cleared by `resume` or a settings change.
    paused_by_error: Option<String>,
}

struct Shared {
    signals: Mutex<Signals>,
    bell: Condvar,
    status: Mutex<Status>,
}

/// Wakes the worker from any thread: after a capture, or a settings change.
#[derive(Clone)]
pub struct Waker(Arc<Shared>);

impl Waker {
    /// A job may be waiting.
    pub fn wake(&self) {
        self.signal(|signals| signals.woken = true);
    }

    /// The settings changed: build the backend again, and lift a pause an error caused.
    pub fn settings_changed(&self) {
        self.signal(|signals| {
            signals.rebuild = true;
            signals.paused_by_error = None;
            signals.woken = true;
        });
    }

    fn signal(&self, change: impl FnOnce(&mut Signals)) {
        if let Ok(mut signals) = self.0.signals.lock() {
            change(&mut signals);
            self.0.bell.notify_all();
        }
    }
}

pub struct Worker {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    /// Starts the worker on the jobs in `queue`, offering candidate notes from `indexes` when
    /// given. `changed` runs on the worker's thread whenever its status or a job changes, to
    /// refresh the tray.
    pub fn start(
        queue: PathBuf,
        factory: Box<dyn Factory>,
        indexes: Option<Indexes>,
        changed: Box<dyn Fn() + Send>,
    ) -> Self {
        let shared = Arc::new(Shared {
            signals: Mutex::new(Signals {
                rebuild: true,
                ..Signals::default()
            }),
            bell: Condvar::new(),
            status: Mutex::new(Status::Idle),
        });
        let worker_shared = shared.clone();
        let thread = std::thread::spawn(move || {
            run(
                &worker_shared,
                &Store::new(queue),
                factory.as_ref(),
                indexes.as_ref(),
                changed.as_ref(),
            );
        });
        Self {
            shared,
            thread: Some(thread),
        }
    }

    pub fn waker(&self) -> Waker {
        Waker(self.shared.clone())
    }

    pub fn status(&self) -> Status {
        self.shared
            .status
            .lock()
            .map(|status| status.clone())
            .unwrap_or(Status::Idle)
    }

    /// Pause processing (the tray menu), or carry on.
    pub fn set_paused(&self, paused: bool) {
        self.waker().signal(|signals| {
            signals.paused_by_user = paused;
            if !paused {
                signals.paused_by_error = None;
            }
            signals.woken = true;
        });
    }

    /// Stops the worker after the job it is on, and waits for it.
    pub fn stop(mut self) {
        self.waker().signal(|signals| signals.stop = true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Ok(mut signals) = self.shared.signals.lock() {
            signals.stop = true;
            self.shared.bell.notify_all();
        }
    }
}

fn set_status(shared: &Shared, status: Status, changed: &dyn Fn()) {
    if let Ok(mut current) = shared.status.lock()
        && *current != status
    {
        *current = status;
        drop(current);
        changed();
    }
}

/// Sleeps until woken or `timeout` passes; returns false when the worker should stop.
fn sleep(shared: &Shared, timeout: Option<Duration>) -> bool {
    let Ok(mut signals) = shared.signals.lock() else {
        return false;
    };
    let deadline = timeout.map(|timeout| std::time::Instant::now() + timeout);
    while !signals.woken && !signals.stop {
        match deadline {
            Some(deadline) => {
                let now = std::time::Instant::now();
                if now >= deadline {
                    break;
                }
                match shared.bell.wait_timeout(signals, deadline - now) {
                    Ok((next, _)) => signals = next,
                    Err(_) => return false,
                }
            }
            None => match shared.bell.wait(signals) {
                Ok(next) => signals = next,
                Err(_) => return false,
            },
        }
    }
    signals.woken = false;
    !signals.stop
}

fn today() -> String {
    let (date, _) = crate::platform::clock::local_now();
    crate::notebook::dates::format(date, "YYYY-MM-DD")
}

fn run(
    shared: &Shared,
    store: &Store,
    factory: &dyn Factory,
    indexes: Option<&Indexes>,
    changed: &dyn Fn(),
) {
    let mut backend: Option<Box<dyn Backend>> = None;
    loop {
        let (stop, rebuild, paused) = match shared.signals.lock() {
            Ok(mut signals) => (
                signals.stop,
                std::mem::take(&mut signals.rebuild),
                if signals.paused_by_user {
                    Some(None)
                } else {
                    signals.paused_by_error.clone().map(Some)
                },
            ),
            Err(_) => return,
        };
        if stop {
            return;
        }
        if rebuild {
            backend = None;
        }
        if let Some(reason) = paused {
            set_status(shared, Status::Paused(reason), changed);
            if !sleep(shared, None) {
                return;
            }
            continue;
        }
        if backend.is_none() {
            match factory.backend() {
                Ok(built) => backend = Some(built),
                Err(reason) => {
                    set_status(shared, Status::Unready(reason), changed);
                    if !sleep(shared, Some(IDLE_CHECK)) {
                        return;
                    }
                    continue;
                }
            }
        }
        let Some(active) = backend.as_deref() else {
            continue;
        };

        let now = process::now_ms();
        let jobs = store.list();
        let Some(job) = jobs.iter().find(|job| job.is_due(now)).cloned() else {
            let soonest = jobs
                .iter()
                .filter(|job| job.state == State::Queued)
                .filter_map(|job| job.retry_at.map(|at| (at, job.last_error.clone())))
                .min_by_key(|(at, _)| *at);
            let wait = match &soonest {
                Some((at, reason)) => {
                    set_status(
                        shared,
                        Status::Waiting {
                            reason: reason.clone().unwrap_or_default(),
                            until: *at,
                        },
                        changed,
                    );
                    process::until(*at, now).min(IDLE_CHECK)
                }
                None => {
                    set_status(shared, Status::Idle, changed);
                    IDLE_CHECK
                }
            };
            if !sleep(shared, Some(wait)) {
                return;
            }
            continue;
        };

        set_status(shared, Status::Working, changed);
        let outcome = process::process_with(store, job, active, now, &today(), indexes);
        match &outcome {
            Outcome::Written(written) => {
                crate::log::line(&format!("runner: wrote {}", written.path.display()));
            }
            Outcome::Retry { reason, .. } => {
                crate::log::line(&format!("runner: will retry: {reason}"))
            }
            Outcome::Failed(reason) => crate::log::line(&format!("runner: failed: {reason}")),
            Outcome::Pause(reason) => {
                crate::log::line(&format!("runner: paused: {reason}"));
                if let Ok(mut signals) = shared.signals.lock() {
                    signals.paused_by_error = Some(reason.clone());
                }
            }
        }
        changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BackendError;
    use crate::runner::process::tests::{Scripted, Temp, note};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// No backend set up: counts how often the worker asked.
    struct Missing(Arc<AtomicUsize>);

    /// The shared script as a backend, so every build answers from the same answers.
    struct Shared2(Arc<Scripted>);

    impl Backend for Shared2 {
        fn label(&self) -> String {
            self.0.label()
        }
        fn complete(
            &self,
            request: &crate::model::request::NoteRequest,
        ) -> Result<crate::note::NoteJson, BackendError> {
            self.0.complete(request)
        }
    }

    impl Factory for Missing {
        fn backend(&self) -> Result<Box<dyn Backend>, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err("Choose a model in Settings".into())
        }
    }

    struct Ready(Arc<Scripted>, Arc<AtomicUsize>);

    impl Factory for Ready {
        fn backend(&self) -> Result<Box<dyn Backend>, String> {
            self.1.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Shared2(self.0.clone())))
        }
    }

    /// Waits up to two seconds for `done`.
    fn eventually(done: impl Fn() -> bool) -> bool {
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < until {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        done()
    }

    #[test]
    fn jobs_become_notes_oldest_first_and_a_wake_brings_new_ones() {
        let temp = Temp::new("worker");
        let first = temp.capture("first");
        let script = Arc::new(Scripted::new(vec![Ok(note("One")), Ok(note("Two"))]));
        let builds = Arc::new(AtomicUsize::new(0));
        let worker = Worker::start(
            temp.0.join("queue"),
            Box::new(Ready(script.clone(), builds.clone())),
            None,
            Box::new(|| {}),
        );
        let store = temp.store();
        assert!(eventually(|| store
            .get(&first.capture_id)
            .is_some_and(|job| job.state == State::Done)));
        let second = temp.capture("second");
        worker.waker().wake();
        assert!(eventually(|| store
            .get(&second.capture_id)
            .is_some_and(|job| job.state == State::Done)));
        assert!(eventually(|| worker.status() == Status::Idle));
        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "one backend for both jobs"
        );
        worker.waker().settings_changed();
        assert!(
            eventually(|| builds.load(Ordering::SeqCst) == 2),
            "rebuilt after a settings change"
        );
        worker.stop();
    }

    #[test]
    fn a_pause_stops_later_jobs_until_resumed() {
        let temp = Temp::new("pause");
        let first = temp.capture("first");
        let script = Arc::new(Scripted::new(vec![
            Err(BackendError::Pause("Claude refused the API key".into())),
            Ok(note("After")),
        ]));
        let worker = Worker::start(
            temp.0.join("queue"),
            Box::new(Ready(script, Arc::new(AtomicUsize::new(0)))),
            None,
            Box::new(|| {}),
        );
        assert!(eventually(
            || worker.status() == Status::Paused(Some("Claude refused the API key".into()))
        ));
        let store = temp.store();
        assert_eq!(store.get(&first.capture_id).unwrap().state, State::Queued);
        worker.set_paused(false);
        assert!(eventually(|| store
            .get(&first.capture_id)
            .is_some_and(|job| job.state == State::Done)));

        worker.set_paused(true);
        assert!(eventually(|| worker.status() == Status::Paused(None)));
        let held = temp.capture("held");
        worker.waker().wake();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            store.get(&held.capture_id).unwrap().state,
            State::Queued,
            "paused by you"
        );
        worker.stop();
    }

    #[test]
    fn without_a_backend_jobs_wait_and_the_status_says_why() {
        let temp = Temp::new("unready");
        let job = temp.capture("x");
        let builds = Arc::new(AtomicUsize::new(0));
        let worker = Worker::start(
            temp.0.join("queue"),
            Box::new(Missing(builds.clone())),
            None,
            Box::new(|| {}),
        );
        assert!(eventually(
            || worker.status() == Status::Unready("Choose a model in Settings".into())
        ));
        assert_eq!(
            temp.store().get(&job.capture_id).unwrap().state,
            State::Queued
        );
        worker.waker().settings_changed();
        assert!(eventually(|| builds.load(Ordering::SeqCst) >= 2));
        worker.stop();
    }

    #[test]
    fn a_retry_shows_as_waiting() {
        let temp = Temp::new("waiting");
        temp.capture("x");
        let script = Arc::new(Scripted::new(vec![Err(BackendError::Retry {
            after: Some(Duration::from_secs(600)),
            reason: "Claude is unavailable (529)".into(),
        })]));
        let worker = Worker::start(
            temp.0.join("queue"),
            Box::new(Ready(script, Arc::new(AtomicUsize::new(0)))),
            None,
            Box::new(|| {}),
        );
        assert!(eventually(|| matches!(
            worker.status(),
            Status::Waiting { ref reason, .. } if reason == "Claude is unavailable (529)"
        )));
        worker.stop();
    }
}
