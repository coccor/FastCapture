//! The worker: one thread that takes queued jobs oldest first and turns them into notes. It
//! sleeps until a capture arrives (`Waker::wake`), a retry falls due, or a minute passes; it
//! stops taking jobs while paused, by you or by an error only you can fix; it builds its
//! backend again when the settings change; and it adds each cloud job's tokens to the usage
//! ledger.

use super::process::{self, Outcome};
use super::usage::today;
use crate::index::service::Indexes;
use crate::model::Backend;
use crate::queue::{State, Store};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The longest the worker sleeps without a reason, in case a job arrived some other way.
const IDLE_CHECK: Duration = Duration::from_secs(60);

/// Builds the backend the settings choose, or says what's missing.
pub trait Factory: Send {
    fn backend(&self) -> Result<Box<dyn Backend>, String>;
    /// The most tokens cloud requests may use in a month, read when a cloud job is due.
    fn token_cap(&self) -> Option<u64> {
        None
    }
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
    /// A limit paused the queue until a time (milliseconds since 1970); it lifts itself then,
    /// or on `resume` or a settings change.
    paused_until: Option<(u64, String)>,
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
            signals.paused_until = None;
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
    /// given and adding cloud jobs' tokens to the ledger at `usage`. `changed` runs on the
    /// worker's thread whenever its status or a job changes, to refresh the tray.
    pub fn start(
        queue: PathBuf,
        factory: Box<dyn Factory>,
        indexes: Option<Indexes>,
        usage: Option<PathBuf>,
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
                usage.as_deref(),
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
                signals.paused_until = None;
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

fn run(
    shared: &Shared,
    store: &Store,
    factory: &dyn Factory,
    indexes: Option<&Indexes>,
    usage: Option<&Path>,
    changed: &dyn Fn(),
) {
    let mut backend: Option<Box<dyn Backend>> = None;
    loop {
        let (stop, rebuild, paused, paused_until) = match shared.signals.lock() {
            Ok(mut signals) => {
                if signals
                    .paused_until
                    .as_ref()
                    .is_some_and(|(until, _)| *until <= process::now_ms())
                {
                    signals.paused_until = None;
                }
                (
                    signals.stop,
                    std::mem::take(&mut signals.rebuild),
                    if signals.paused_by_user {
                        Some(None)
                    } else {
                        signals.paused_by_error.clone().map(Some)
                    },
                    signals.paused_until.clone(),
                )
            }
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
        if let Some((until, reason)) = paused_until {
            set_status(shared, Status::Paused(Some(reason)), changed);
            let wait = process::until(until, process::now_ms()).min(IDLE_CHECK);
            if !sleep(shared, Some(wait)) {
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

        let day = today();
        if let Some(reason) = cap_reached(active, factory, usage, &day) {
            set_status(shared, Status::Paused(Some(reason)), changed);
            if !sleep(shared, Some(IDLE_CHECK)) {
                return;
            }
            continue;
        }
        set_status(shared, Status::Working, changed);
        let outcome = process::process_with(store, job, active, now, &day, indexes);
        record_usage(active, usage, &day);
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
            Outcome::PauseUntil { until, reason } => {
                crate::log::line(&format!("runner: paused until {until}: {reason}"));
                if let Ok(mut signals) = shared.signals.lock() {
                    signals.paused_until = Some((*until, reason.clone()));
                }
            }
        }
        changed();
    }
}

/// Why a cloud job must wait: this month's tokens have reached the cap. Checked again at each
/// wake and idle check, so raising the cap or a new month lets jobs run.
fn cap_reached(
    backend: &dyn Backend,
    factory: &dyn Factory,
    ledger: Option<&Path>,
    day: &str,
) -> Option<String> {
    if !backend.metered() {
        return None;
    }
    let cap = factory.token_cap()?;
    super::usage::cap_reached(&super::usage::Ledger::load(ledger?), day, cap)
}

/// Adds what a metered backend's last job used to the ledger.
fn record_usage(backend: &dyn Backend, ledger: Option<&Path>, day: &str) {
    let Some(used) = backend.take_usage() else {
        return;
    };
    let label = backend.label();
    if let Some(ledger) = ledger
        && backend.metered()
        && let Err(error) = super::usage::record(ledger, day, &label, used)
    {
        crate::log::line(&format!("usage: {error}"));
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

    /// A backend that used 100 input and 10 output tokens per request.
    struct Metered {
        label: &'static str,
        script: Scripted,
        meter: crate::model::backend::Meter,
    }

    impl Backend for Metered {
        fn label(&self) -> String {
            self.label.to_owned()
        }
        fn complete(
            &self,
            request: &crate::model::request::NoteRequest,
        ) -> Result<crate::note::NoteJson, BackendError> {
            self.meter.add(crate::model::Usage {
                input: 100,
                output: 10,
                ..Default::default()
            });
            self.script.complete(request)
        }
        fn take_usage(&self) -> Option<crate::model::Usage> {
            self.meter.take()
        }
    }

    struct Once(Mutex<Option<Metered>>, Arc<std::sync::atomic::AtomicU64>);

    impl Factory for Once {
        fn backend(&self) -> Result<Box<dyn Backend>, String> {
            self.0
                .lock()
                .unwrap()
                .take()
                .map(|backend| Box::new(backend) as Box<dyn Backend>)
                .ok_or_else(|| "built once".to_owned())
        }
        fn token_cap(&self) -> Option<u64> {
            Some(self.1.load(Ordering::SeqCst)).filter(|cap| *cap > 0)
        }
    }

    fn metered(
        label: &'static str,
        answers: Vec<Result<crate::note::NoteJson, BackendError>>,
    ) -> Box<Once> {
        capped(label, answers, Arc::default())
    }

    fn capped(
        label: &'static str,
        answers: Vec<Result<crate::note::NoteJson, BackendError>>,
        cap: Arc<std::sync::atomic::AtomicU64>,
    ) -> Box<Once> {
        Box::new(Once(
            Mutex::new(Some(Metered {
                label,
                script: Scripted::new(answers),
                meter: Default::default(),
            })),
            cap,
        ))
    }

    #[test]
    fn a_cloud_jobs_tokens_go_in_the_ledger_both_requests_of_a_second_ask() {
        let temp = Temp::new("usage");
        let job = temp.capture("first");
        let ledger = temp.0.join(super::super::usage::FILE);
        let unusable =
            BackendError::Invalid(crate::note::reply::ReplyError(vec!["no title".into()]));
        let worker = Worker::start(
            temp.0.join("queue"),
            metered("cloud/test-model", vec![Err(unusable), Ok(note("One"))]),
            None,
            Some(ledger.clone()),
            Box::new(|| {}),
        );
        let store = temp.store();
        assert!(eventually(|| store
            .get(&job.capture_id)
            .is_some_and(|job| job.state == State::Done)));
        assert!(eventually(|| worker.status() == Status::Idle));
        worker.stop();
        let used = super::super::usage::Ledger::load(&ledger).day(&today());
        assert_eq!((used.input, used.output), (200, 20));
        assert!(
            std::fs::read_to_string(&ledger)
                .unwrap()
                .contains(" cloud/test-model ")
        );
    }

    #[test]
    fn a_cloud_job_waits_once_the_month_reaches_the_cap_until_it_is_raised() {
        let temp = Temp::new("cap");
        let ledger = temp.0.join(super::super::usage::FILE);
        super::super::usage::record(
            &ledger,
            &today(),
            "cloud/test-model",
            crate::model::Usage {
                input: 900,
                output: 100,
                ..Default::default()
            },
        )
        .unwrap();
        let job = temp.capture("first");
        let cap = Arc::new(std::sync::atomic::AtomicU64::new(1000));
        let worker = Worker::start(
            temp.0.join("queue"),
            capped("cloud/test-model", vec![Ok(note("One"))], cap.clone()),
            None,
            Some(ledger.clone()),
            Box::new(|| {}),
        );
        assert!(eventually(|| matches!(
            worker.status(),
            Status::Paused(Some(ref reason)) if reason.starts_with("This month's token cap (1,000) is reached")
        )));
        let store = temp.store();
        assert_eq!(store.get(&job.capture_id).unwrap().state, State::Queued);
        cap.store(2000, Ordering::SeqCst);
        worker.waker().wake();
        assert!(eventually(|| store
            .get(&job.capture_id)
            .is_some_and(|job| job.state == State::Done)));
        worker.stop();
    }

    #[test]
    fn local_mode_isnt_capped() {
        let temp = Temp::new("cap-local");
        let ledger = temp.0.join(super::super::usage::FILE);
        super::super::usage::record(
            &ledger,
            &today(),
            "cloud/test-model",
            crate::model::Usage {
                input: 5000,
                ..Default::default()
            },
        )
        .unwrap();
        let job = temp.capture("first");
        let worker = Worker::start(
            temp.0.join("queue"),
            capped(
                "local/test-model",
                vec![Ok(note("One"))],
                Arc::new(std::sync::atomic::AtomicU64::new(1000)),
            ),
            None,
            Some(ledger),
            Box::new(|| {}),
        );
        let store = temp.store();
        assert!(eventually(|| store
            .get(&job.capture_id)
            .is_some_and(|job| job.state == State::Done)));
        worker.stop();
    }

    #[test]
    fn a_local_servers_tokens_arent_counted() {
        let temp = Temp::new("usage-local");
        let job = temp.capture("first");
        let ledger = temp.0.join(super::super::usage::FILE);
        let worker = Worker::start(
            temp.0.join("queue"),
            metered("local/test-model", vec![Ok(note("One"))]),
            None,
            Some(ledger.clone()),
            Box::new(|| {}),
        );
        let store = temp.store();
        assert!(eventually(|| store
            .get(&job.capture_id)
            .is_some_and(|job| job.state == State::Done)));
        worker.stop();
        assert!(!ledger.exists());
    }

    /// Waits up to ten seconds for `done`, as a shared CI runner can be slow.
    fn eventually(done: impl Fn() -> bool) -> bool {
        let until = std::time::Instant::now() + Duration::from_secs(10);
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
    fn a_limit_pauses_the_queue_until_its_time_then_it_goes_on() {
        let temp = Temp::new("pause-until");
        let first = temp.capture("first");
        let until = process::now_ms() + 2_000;
        let script = Arc::new(Scripted::new(vec![
            Err(BackendError::PauseUntil {
                until,
                reason: "The usage limit is reached".into(),
            }),
            Ok(note("After")),
        ]));
        let worker = Worker::start(
            temp.0.join("queue"),
            Box::new(Ready(script, Arc::new(AtomicUsize::new(0)))),
            None,
            None,
            Box::new(|| {}),
        );
        assert!(eventually(
            || worker.status() == Status::Paused(Some("The usage limit is reached".into()))
        ));
        let store = temp.store();
        assert_eq!(store.get(&first.capture_id).unwrap().state, State::Queued);
        assert!(
            eventually(|| store
                .get(&first.capture_id)
                .is_some_and(|job| job.state == State::Done)),
            "it goes on by itself"
        );
        assert!(process::now_ms() >= until);
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
