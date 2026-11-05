//! The indexes of the notebooks in use, one thread each: read from the cache, brought up to date
//! by a scan, then kept current by a watcher. The worker asks for a notebook's snapshot, waiting
//! while its first build runs; a notebook that can no longer be read is dropped, and indexed
//! again the next time it is asked for.

use super::notes::{self, NoteIndex};
use super::scan::{self, NOTE_LIMIT};
use super::search::Searcher;
use super::watch::Watcher;
use crate::notebook::Conventions;
use crate::notebook::obsidian::ObsidianAttachments;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long the worker waits for a notebook's first build before writing a note without
/// candidates.
pub const FIRST_BUILD_WAIT: Duration = Duration::from_secs(10);
/// How long to wait before watching a notebook again after its watch failed, by attempt; the
/// last delay repeats.
#[cfg(not(test))]
const REWATCH_DELAYS: [Duration; 4] = [
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(120),
    Duration::from_secs(600),
];
#[cfg(test)]
const REWATCH_DELAYS: [Duration; 4] = [Duration::from_millis(50); 4];
/// A watcher that lasted this long has earned the shortest delay again when it fails.
const WATCH_HEALTHY: Duration = Duration::from_secs(600);
/// With nothing watching, the notebook is rescanned this often.
const RESCAN_EVERY: Duration = Duration::from_secs(3600);
/// A changed index is written to its cache at most this often, and when the service stops.
const CACHE_EVERY: Duration = Duration::from_secs(60);

/// A notebook's index as of its last scan.
pub struct Snapshot {
    pub searcher: Searcher,
    /// Every folder in the notebook, relative to it, as the scan listed them.
    pub folders: Vec<PathBuf>,
    /// The notebook has more notes than are indexed.
    pub truncated: bool,
}

#[derive(Default)]
struct State {
    snapshot: Option<Arc<Snapshot>>,
    dirty: bool,
    /// The watcher reported that it failed and stopped.
    watch_failed: bool,
    /// Try to watch again now, if nothing is watching.
    rewatch_now: bool,
    stop: bool,
    /// The notebook can no longer be read; its thread has ended.
    gone: bool,
}

struct Notebook {
    root: PathBuf,
    state: Mutex<State>,
    bell: Condvar,
    /// Snapshots published while a watcher was running.
    #[cfg(test)]
    watched_builds: std::sync::atomic::AtomicUsize,
    /// Makes the wait before watching again an hour.
    #[cfg(test)]
    slow_rewatch: std::sync::atomic::AtomicBool,
}

impl Notebook {
    fn change(&self, change: impl FnOnce(&mut State)) {
        if let Ok(mut state) = self.state.lock() {
            change(&mut state);
            self.bell.notify_all();
        }
    }
}

/// A notebook being indexed, and its thread.
type Running = (Arc<Notebook>, Option<JoinHandle<()>>);

struct Shared {
    /// `%LocalAppData%\FastCapture`, where caches go; `None` keeps no cache.
    local: Option<PathBuf>,
    /// By `notes::folder_key`.
    notebooks: Mutex<HashMap<String, Running>>,
}

/// The indexes, shared by the tray app and the worker.
#[derive(Clone)]
pub struct Indexes(Arc<Shared>);

impl Indexes {
    /// No notebook is indexed until `open` or `snapshot` asks for it.
    pub fn new(local: Option<PathBuf>) -> Self {
        Self(Arc::new(Shared {
            local,
            notebooks: Mutex::new(HashMap::new()),
        }))
    }

    /// Starts indexing `root`, unless it already is or isn't a folder.
    pub fn open(&self, root: &Path) {
        self.notebook(root);
    }

    /// `root`'s index, waiting up to `wait` for its first build; `None` when it isn't ready by
    /// then or can't be read.
    pub fn snapshot(&self, root: &Path, wait: Duration) -> Option<Arc<Snapshot>> {
        let notebook = self.notebook(root)?;
        let deadline = Instant::now() + wait;
        let mut state = notebook.state.lock().ok()?;
        loop {
            if let Some(snapshot) = &state.snapshot {
                return Some(snapshot.clone());
            }
            if state.gone || state.stop {
                return None;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = notebook.bell.wait_timeout(state, left).ok()?.0;
        }
    }

    /// Marks every open notebook for a rescan, and asks the ones nothing watches to try watching
    /// again now.
    pub fn refresh_all(&self) {
        let open: Vec<_> = match self.0.notebooks.lock() {
            Ok(notebooks) => notebooks.values().map(|(n, _)| n.clone()).collect(),
            Err(_) => return,
        };
        for notebook in open {
            notebook.change(|state| {
                state.dirty = true;
                state.rewatch_now = true;
            });
        }
    }

    /// What a failed batch from the watcher does to `root`'s notebook.
    #[cfg(test)]
    pub(crate) fn fail_watch_for_test(&self, root: &Path) {
        if let Some(notebook) = self.notebook(root) {
            notebook.change(|s| {
                s.watch_failed = true;
                s.dirty = true;
            });
        }
    }

    /// Whether `root` is being indexed.
    pub fn is_open(&self, root: &Path) -> bool {
        self.0
            .notebooks
            .lock()
            .is_ok_and(|notebooks| notebooks.contains_key(&notes::folder_key(root)))
    }

    /// Stops every index, writing the ones that changed to their caches, and waits for them.
    pub fn stop(&self) {
        let drained: Vec<_> = match self.0.notebooks.lock() {
            Ok(mut notebooks) => notebooks.drain().map(|(_, entry)| entry).collect(),
            Err(_) => return,
        };
        for (notebook, _) in &drained {
            notebook.change(|state| state.stop = true);
        }
        for (_, thread) in drained {
            if let Some(thread) = thread {
                let _ = thread.join();
            }
        }
    }

    /// How many notes `root`'s index holds, when it is already open and built. Never opens a
    /// notebook, reads the disk or waits.
    pub fn notes_if_open(&self, root: &Path) -> Option<usize> {
        let key = notes::folder_key(root);
        let notebook = self.0.notebooks.lock().ok()?.get(&key)?.0.clone();
        let state = notebook.state.try_lock().ok()?;
        state
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.searcher.index().notes.len())
    }

    fn notebook(&self, root: &Path) -> Option<Arc<Notebook>> {
        let key = notes::folder_key(root);
        let mut notebooks = self.0.notebooks.lock().ok()?;
        if let Some((notebook, _)) = notebooks.get(&key) {
            return Some(notebook.clone());
        }
        if !root.is_dir() {
            return None;
        }
        let notebook = Arc::new(Notebook {
            root: root.to_path_buf(),
            state: Mutex::new(State::default()),
            bell: Condvar::new(),
            #[cfg(test)]
            watched_builds: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            slow_rewatch: std::sync::atomic::AtomicBool::new(false),
        });
        let cache = self
            .0
            .local
            .as_deref()
            .map(|local| notes::cache_file(local, root));
        let thread_notebook = notebook.clone();
        let shared = Arc::downgrade(&self.0);
        let thread = std::thread::Builder::new()
            .name("fastcapture-index".to_owned())
            .spawn(move || run(&thread_notebook, cache.as_deref(), &shared))
            .ok()?;
        notebooks.insert(key, (notebook.clone(), Some(thread)));
        Some(notebook)
    }
}

/// The folders whose notes aren't the notebook's own: raw sources, templates, and the
/// attachments folder when it is one fixed folder.
fn left_out(conventions: &Conventions) -> Vec<PathBuf> {
    let mut folders = vec![
        conventions.raw_folder().to_path_buf(),
        conventions.notebook.templates_folder.clone(),
    ];
    let per_note = conventions.notebook.attachments_folder.is_none()
        && matches!(
            conventions.obsidian,
            Some(ObsidianAttachments::NextToNote | ObsidianAttachments::UnderNote(_))
        );
    let attachments = conventions.attachments_folder_for(Path::new("note.md"));
    if !per_note && !attachments.as_os_str().is_empty() {
        folders.push(attachments);
    }
    folders
}

/// One notebook's thread: builds, publishes and waits for changes until stopped or gone.
fn run(notebook: &Arc<Notebook>, cache: Option<&Path>, shared: &Weak<Shared>) {
    let start_watcher = || {
        let watched = Arc::downgrade(notebook);
        Watcher::start(
            notebook.root.clone(),
            Box::new(move |changes| {
                if let Some(notebook) = watched.upgrade() {
                    notebook.change(|state| {
                        state.dirty = true;
                        state.watch_failed |= changes.failed;
                    });
                }
            }),
        )
    };
    let Ok(first) = start_watcher() else {
        drop_gone(notebook, shared);
        return;
    };
    let mut watcher = Some(first);
    let mut watching_since = Instant::now();
    let mut rewatch_at: Option<Instant> = None;
    let mut attempts = 0usize;
    let delay = |attempts: usize| {
        #[cfg(test)]
        if notebook
            .slow_rewatch
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Duration::from_secs(3600);
        }
        REWATCH_DELAYS[attempts.min(3)]
    };
    let mut previous = cache.map(notes::read_cache).unwrap_or_default();
    let mut unsaved = false;
    let mut saved_at: Option<Instant> = None;
    let save = |index: &NoteIndex, saved_at: &mut Option<Instant>| {
        if let Some(cache) = cache
            && let Err(error) = notes::write_cache(cache, index)
        {
            crate::log::line(&format!("index: {}: {error}", cache.display()));
        }
        *saved_at = Some(Instant::now());
    };
    loop {
        let (failed, retry_now) = {
            let Ok(mut state) = notebook.state.lock() else {
                return;
            };
            state.dirty = false;
            (
                std::mem::take(&mut state.watch_failed),
                std::mem::take(&mut state.rewatch_now),
            )
        };
        if failed {
            // The old watcher is dead; it goes before a new one starts.
            watcher = None;
            if watching_since.elapsed() >= WATCH_HEALTHY {
                attempts = 0;
            }
            crate::log::line("index: watching stopped; will try again");
            rewatch_at = Some(Instant::now() + delay(attempts));
            attempts += 1;
        }
        if watcher.is_none() && (retry_now || rewatch_at.is_some_and(|at| at <= Instant::now())) {
            match start_watcher() {
                Ok(started) => {
                    watcher = Some(started);
                    watching_since = Instant::now();
                    rewatch_at = None;
                }
                Err(_) => {
                    rewatch_at = Some(Instant::now() + delay(attempts));
                    attempts += 1;
                }
            }
        }
        let conventions = Conventions::load(&notebook.root);
        let Ok(scan) = scan::scan(&notebook.root, &left_out(&conventions), NOTE_LIMIT) else {
            // The folder's handle goes first, so a folder being deleted can be made again.
            drop(watcher.take());
            drop_gone(notebook, shared);
            return;
        };
        let (index, changed) = NoteIndex::build(&notebook.root, &scan.entries, previous);
        unsaved |= changed;
        if unsaved && saved_at.is_none_or(|at| at.elapsed() >= CACHE_EVERY) {
            save(&index, &mut saved_at);
            unsaved = false;
        }
        previous = index.clone();
        let snapshot = Arc::new(Snapshot {
            searcher: Searcher::new(index),
            folders: scan.folders,
            truncated: scan.truncated,
        });
        notebook.change(|state| state.snapshot = Some(snapshot));
        #[cfg(test)]
        if watcher.is_some() {
            notebook
                .watched_builds
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        // Wait for a change or the stop, writing the cache when it falls due meanwhile, and
        // waking to watch again or, with no watcher, to rescan.
        let Ok(mut state) = notebook.state.lock() else {
            return;
        };
        let mut rescan_at = Instant::now() + RESCAN_EVERY;
        while !state.dirty && !state.stop {
            let save_due = match (unsaved, saved_at) {
                (true, Some(at)) => Some(at + CACHE_EVERY),
                _ => None,
            };
            let wake = [save_due, rewatch_at, Some(rescan_at)]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or(rescan_at);
            let Ok((next, _)) = notebook
                .bell
                .wait_timeout(state, wake.saturating_duration_since(Instant::now()))
            else {
                return;
            };
            state = next;
            let now = Instant::now();
            if unsaved && save_due.is_some_and(|at| at <= now) {
                drop(state);
                save(&previous, &mut saved_at);
                unsaved = false;
                let Ok(next) = notebook.state.lock() else {
                    return;
                };
                state = next;
            }
            if watcher.is_none() && (rewatch_at.is_some_and(|at| at <= now) || rescan_at <= now) {
                break;
            }
            if rescan_at <= now {
                // Watched, so the hour passing asks for nothing; the next wait starts afresh.
                rescan_at = now + RESCAN_EVERY;
            }
        }
        if state.stop {
            drop(state);
            if unsaved {
                save(&previous, &mut saved_at);
            }
            return;
        }
    }
}

/// Marks `notebook` gone, wakes whoever waits on it, and forgets it.
fn drop_gone(notebook: &Arc<Notebook>, shared: &Weak<Shared>) {
    notebook.change(|state| {
        state.gone = true;
        state.snapshot = None;
    });
    if let Some(shared) = shared.upgrade()
        && let Ok(mut notebooks) = shared.notebooks.lock()
    {
        let key = notes::folder_key(&notebook.root);
        if notebooks
            .get(&key)
            .is_some_and(|(known, _)| Arc::ptr_eq(known, notebook))
        {
            // The thread is this one, ending now: nothing to join.
            notebooks.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::scan::tests::Scratch;

    /// Waits up to `within` for `found` to hold of `root`'s snapshot.
    fn eventually(
        indexes: &Indexes,
        root: &Path,
        within: Duration,
        found: impl Fn(&Snapshot) -> bool,
    ) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if indexes
                .snapshot(root, Duration::from_millis(100))
                .is_some_and(|snapshot| found(&snapshot))
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn finds(snapshot: &Snapshot, query: &str, path: &str) -> bool {
        snapshot
            .searcher
            .search(query, 30)
            .iter()
            .any(|hit| hit.note.path == Path::new(path))
    }

    #[test]
    fn a_note_added_is_found_within_a_second() {
        let scratch = Scratch::new("service-add");
        scratch.file(r"Payments\Ledger.md", "# Ledger batch\n\nkafka lag");
        scratch.file(r"raw\2026-10-19 capture.md", "kafka lag in raw");
        let indexes = Indexes::new(None);
        let snapshot = indexes.snapshot(&scratch.0, FIRST_BUILD_WAIT).unwrap();
        assert!(finds(&snapshot, "kafka", r"Payments\Ledger.md"));
        assert!(!finds(&snapshot, "kafka", r"raw\2026-10-19 capture.md"));
        assert!(snapshot.folders.contains(&PathBuf::from("Payments")));

        let started = Instant::now();
        scratch.file(
            r"Payments\Rebalance.md",
            "# Rebalancing\n\nconsumer rebalance",
        );
        assert!(eventually(
            &indexes,
            &scratch.0,
            Duration::from_secs(1),
            |snapshot| finds(snapshot, "rebalance", r"Payments\Rebalance.md")
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        indexes.stop();
        assert!(!indexes.is_open(&scratch.0));
    }

    #[test]
    fn a_query_while_the_first_build_runs_waits_for_it() {
        let scratch = Scratch::new("service-wait");
        for number in 0..500 {
            scratch.file(
                &format!(r"notes\n{number}.md"),
                &format!("# Note {number}\n\nledger kafka payments {number}"),
            );
        }
        scratch.file(r"notes\target.md", "# Target\n\nzanzibar");
        let indexes = Indexes::new(None);
        indexes.open(&scratch.0);
        // Asked at once, while the build still runs: the answer waits for it.
        let snapshot = indexes.snapshot(&scratch.0, FIRST_BUILD_WAIT).unwrap();
        assert!(finds(&snapshot, "zanzibar", r"notes\target.md"));
        assert_eq!(snapshot.searcher.index().notes.len(), 501);
        indexes.stop();
    }

    #[test]
    fn the_cache_is_written_on_stop_and_read_on_start() {
        let scratch = Scratch::new("service-cache");
        let local = scratch.0.join("local");
        let notebook = scratch.0.join("book");
        std::fs::create_dir_all(&notebook).unwrap();
        std::fs::write(notebook.join("a.md"), "# Alpha\n\nkafka").unwrap();
        let indexes = Indexes::new(Some(local.clone()));
        assert!(indexes.snapshot(&notebook, FIRST_BUILD_WAIT).is_some());
        indexes.stop();
        let cached = notes::read_cache(&notes::cache_file(&local, &notebook));
        assert_eq!(cached.notes.len(), 1);
    }

    #[test]
    fn counting_notes_never_opens_a_notebook() {
        let scratch = Scratch::new("service-count");
        let notebook = scratch.0.join("book");
        std::fs::create_dir_all(&notebook).unwrap();
        std::fs::write(notebook.join("a.md"), "# Alpha").unwrap();
        let indexes = Indexes::new(None);
        assert_eq!(indexes.notes_if_open(&notebook), None);
        assert!(!indexes.is_open(&notebook));
        assert!(indexes.snapshot(&notebook, FIRST_BUILD_WAIT).is_some());
        assert_eq!(indexes.notes_if_open(&notebook), Some(1));
        indexes.stop();
    }

    #[test]
    fn a_notebook_that_disappears_is_dropped() {
        let scratch = Scratch::new("service-gone");
        let notebook = scratch.0.join("book");
        std::fs::create_dir_all(&notebook).unwrap();
        std::fs::write(notebook.join("a.md"), "# Alpha").unwrap();
        let indexes = Indexes::new(None);
        assert!(indexes.snapshot(&notebook, FIRST_BUILD_WAIT).is_some());
        assert!(indexes.is_open(&notebook));

        std::fs::remove_dir_all(&notebook).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while indexes.is_open(&notebook) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!indexes.is_open(&notebook));
        assert!(indexes.snapshot(&notebook, Duration::ZERO).is_none());

        // Back again, it is indexed again when asked for.
        std::fs::create_dir_all(&notebook).unwrap();
        std::fs::write(notebook.join("b.md"), "# Beta").unwrap();
        let snapshot = indexes.snapshot(&notebook, FIRST_BUILD_WAIT).unwrap();
        assert!(finds(&snapshot, "beta", "b.md"));
        indexes.stop();
    }

    /// Waits up to `within` for `root`'s notebook to have published `count` snapshots while
    /// watched.
    fn watched_builds_reach(
        indexes: &Indexes,
        root: &Path,
        count: usize,
        within: Duration,
    ) -> bool {
        let notebook = indexes.notebook(root).unwrap();
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            let builds = notebook
                .watched_builds
                .load(std::sync::atomic::Ordering::SeqCst);
            if builds >= count {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn a_notebook_whose_watch_failed_is_watched_again() {
        let scratch = Scratch::new("rewatch");
        scratch.file("a.md", "# Alpha");
        let indexes = Indexes::new(None);
        indexes.open(&scratch.0);
        let within = Duration::from_secs(5);
        assert!(watched_builds_reach(&indexes, &scratch.0, 1, within));
        indexes.fail_watch_for_test(&scratch.0);
        // Watched again, and the rescan that follows has finished: a note written now can only
        // be seen by the new watcher.
        assert!(watched_builds_reach(&indexes, &scratch.0, 2, within));
        scratch.file("c.md", "# Charlie");
        assert!(eventually(&indexes, &scratch.0, within, |s| finds(
            s, "charlie", "c.md"
        )));
        indexes.stop();
    }

    #[test]
    fn a_refresh_watches_again_without_waiting_for_the_delay() {
        let scratch = Scratch::new("refresh");
        scratch.file("a.md", "# Alpha");
        let indexes = Indexes::new(None);
        indexes.open(&scratch.0);
        let within = Duration::from_secs(5);
        assert!(watched_builds_reach(&indexes, &scratch.0, 1, within));
        indexes
            .notebook(&scratch.0)
            .unwrap()
            .slow_rewatch
            .store(true, std::sync::atomic::Ordering::SeqCst);
        indexes.fail_watch_for_test(&scratch.0);
        indexes.refresh_all();
        // The delay is an hour: only the refresh can have brought the watcher back.
        assert!(watched_builds_reach(&indexes, &scratch.0, 2, within));
        scratch.file("c.md", "# Charlie");
        assert!(eventually(&indexes, &scratch.0, within, |s| finds(
            s, "charlie", "c.md"
        )));
        indexes.stop();
    }

    #[test]
    fn a_missing_folder_is_not_indexed() {
        let indexes = Indexes::new(None);
        let missing = std::env::temp_dir().join("fastcapture-no-such-notebook");
        assert!(indexes.snapshot(&missing, Duration::from_secs(1)).is_none());
        assert!(!indexes.is_open(&missing));
    }

    #[test]
    fn raw_templates_and_attachments_are_left_out() {
        let conventions = Conventions::load(Path::new(r"C:\no-such-notebook"));
        assert_eq!(
            left_out(&conventions),
            [
                PathBuf::from("raw"),
                PathBuf::from(r".fastpad\templates"),
                PathBuf::from("attachments")
            ]
        );
    }
}
