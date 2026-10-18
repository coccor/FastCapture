//! The per-PC queue: one JSON file per job in `%LocalAppData%\FastCapture\queue`,
//! named by its capture id and written atomically. It survives restarts: on start, jobs left
//! `working` go back to `queued`, and raw sources with no job get one.

use super::job::{Job, State};
use crate::notebook::Conventions;
use crate::notebook::frontmatter::{self, Value};
use crate::platform::saver;
use std::path::{Path, PathBuf};

/// `%LocalAppData%\FastCapture\queue`.
pub fn default_dir() -> Option<PathBuf> {
    crate::paths::local().map(|local| local.join("queue"))
}

/// A capture id safe to use as a file name: letters and digits only, 1 to 64 of them. Ids in
/// raw sources may have been edited by hand.
pub fn is_valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path_of(&self, capture_id: &str) -> PathBuf {
        self.dir.join(format!("{capture_id}.json"))
    }

    /// Writes `job`, replacing any earlier state of the same capture.
    pub fn put(&self, job: &Job) -> Result<(), String> {
        if !is_valid_id(&job.capture_id) {
            return Err(format!("invalid capture id \"{}\"", job.capture_id));
        }
        std::fs::create_dir_all(&self.dir).map_err(|error| {
            format!(
                "FastCapture could not create the folder {}: {error}",
                self.dir.display()
            )
        })?;
        let path = self.path_of(&job.capture_id);
        saver::save_atomic(&path, job.to_json().as_bytes())
            .map_err(|error| format!("FastCapture could not save {}: {error}", path.display()))
    }

    /// The job for `capture_id`, if there is one that can be read.
    pub fn get(&self, capture_id: &str) -> Option<Job> {
        if !is_valid_id(capture_id) {
            return None;
        }
        let text = std::fs::read_to_string(self.path_of(capture_id)).ok()?;
        Job::from_json(&text).ok()
    }

    /// Every job, oldest first. A file that isn't a job is renamed to `.bad` and logged, so it is
    /// looked at once and never stops the queue.
    pub fn list(&self) -> Vec<Job> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut jobs: Vec<Job> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .filter_map(|path| {
                let text = std::fs::read_to_string(&path).ok()?;
                match Job::from_json(&text) {
                    Ok(job) => Some(job),
                    Err(error) => {
                        crate::log::line(&format!(
                            "queue: {} is not a job ({error}); moved aside",
                            path.display()
                        ));
                        let _ = std::fs::rename(&path, path.with_extension("bad"));
                        None
                    }
                }
            })
            .collect();
        jobs.sort_by(|a, b| {
            (a.created.as_str(), a.capture_id.as_str())
                .cmp(&(b.created.as_str(), b.capture_id.as_str()))
        });
        jobs
    }

    /// Changes a job's state, counting an attempt when it starts working.
    pub fn set_state(
        &self,
        capture_id: &str,
        state: State,
        error: Option<String>,
    ) -> Result<Job, String> {
        let mut job = self
            .get(capture_id)
            .ok_or_else(|| format!("no job for capture {capture_id}"))?;
        if state == State::Working {
            job.attempts += 1;
        }
        job.state = state;
        job.last_error = error;
        self.put(&job)?;
        Ok(job)
    }

    /// Removes a job (its capture was discarded).
    pub fn remove(&self, capture_id: &str) {
        if is_valid_id(capture_id) {
            let _ = std::fs::remove_file(self.path_of(capture_id));
        }
    }

    /// After a restart: jobs left `working` were interrupted, so they are queued again. Returns
    /// how many.
    pub fn recover(&self) -> usize {
        let mut recovered = 0;
        for job in self.list() {
            if job.state == State::Working
                && self
                    .set_state(&job.capture_id, State::Queued, job.last_error.clone())
                    .is_ok()
            {
                recovered += 1;
            }
        }
        recovered
    }

    /// Raw sources in the notebook's raw folder that have no job (written by `fastcapture add`
    /// while the tray app was closed, or copied from another PC) get a queued job. Returns the
    /// jobs added.
    pub fn enqueue_orphans(&self, conventions: &Conventions) -> Vec<Job> {
        let mut added = Vec::new();
        for raw in raw_sources(&conventions.root, conventions.raw_folder()) {
            if self.get(&raw.capture_id).is_some() {
                continue;
            }
            let job = Job::queued(
                &raw.capture_id,
                conventions.root.clone(),
                raw.path,
                raw.captured,
            );
            match self.put(&job) {
                Ok(()) => added.push(job),
                Err(error) => crate::log::line(&format!("queue: {error}")),
            }
        }
        added
    }
}

/// A raw source found on disk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawFound {
    /// Relative to the notebook.
    pub path: PathBuf,
    pub capture_id: String,
    pub captured: String,
}

/// The raw sources directly in `raw_folder`: Markdown files whose front matter has `type: raw`
/// and a valid `capture_id`. Sorted by path.
pub fn raw_sources(root: &Path, raw_folder: &Path) -> Vec<RawFound> {
    let Ok(entries) = std::fs::read_dir(root.join(raw_folder)) else {
        return Vec::new();
    };
    let mut found: Vec<RawFound> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        })
        .filter_map(|path| {
            let text = read_head(&path)?;
            let front = frontmatter::parse(&text)?;
            let text_of = |key: &str| match front.get(key) {
                Some(Value::Text(value)) => Some(value.clone()),
                _ => None,
            };
            if !text_of("type")?.eq_ignore_ascii_case("raw") {
                return None;
            }
            let capture_id = text_of("capture_id").filter(|id| is_valid_id(id))?;
            Some(RawFound {
                path: raw_folder.join(path.file_name()?),
                capture_id,
                captured: text_of("captured").unwrap_or_default(),
            })
        })
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// The first 16 KB of a file as text, enough for its front matter.
fn read_head(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(16 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    Some(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("fastcapture-queue-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn job(id: &str, created: &str) -> Job {
        Job::queued(
            id,
            PathBuf::from(r"C:\Notes"),
            PathBuf::from(format!(r"raw\{id}.md")),
            created.to_owned(),
        )
    }

    #[test]
    fn jobs_are_listed_oldest_first_and_change_state() {
        let temp = Temp::new("list");
        let store = Store::new(temp.0.join("queue"));
        assert!(store.list().is_empty(), "no folder yet");
        store.put(&job("B", "2026-10-19T10:31")).unwrap();
        store.put(&job("A", "2026-10-19T10:31")).unwrap();
        store.put(&job("C", "2026-10-19T09:00")).unwrap();
        let ids: Vec<String> = store.list().into_iter().map(|job| job.capture_id).collect();
        assert_eq!(ids, ["C", "A", "B"]);

        let working = store.set_state("A", State::Working, None).unwrap();
        assert_eq!(working.attempts, 1);
        let failed = store
            .set_state("A", State::Failed, Some("timeout".to_owned()))
            .unwrap();
        assert_eq!((failed.state, failed.attempts), (State::Failed, 1));
        assert_eq!(
            store.get("A").unwrap().last_error.as_deref(),
            Some("timeout")
        );
        assert!(store.set_state("Z", State::Done, None).is_err());

        store.remove("B");
        assert_eq!(store.list().len(), 2);
    }

    #[test]
    fn interrupted_jobs_are_queued_again() {
        let temp = Temp::new("recover");
        let store = Store::new(temp.0.clone());
        store.put(&job("A", "1")).unwrap();
        store.put(&job("B", "2")).unwrap();
        store.set_state("A", State::Working, None).unwrap();
        store
            .set_state("B", State::Failed, Some("bad key".into()))
            .unwrap();
        assert_eq!(store.recover(), 1);
        assert_eq!(store.get("A").unwrap().state, State::Queued);
        assert_eq!(store.get("A").unwrap().attempts, 1);
        assert_eq!(store.get("B").unwrap().state, State::Failed);
    }

    #[test]
    fn a_broken_job_file_is_moved_aside() {
        let temp = Temp::new("broken");
        let store = Store::new(temp.0.clone());
        store.put(&job("A", "1")).unwrap();
        fs::write(temp.0.join("X.json"), b"{ not json").unwrap();
        assert_eq!(store.list().len(), 1);
        assert!(temp.0.join("X.bad").exists());
        assert!(!temp.0.join("X.json").exists());
    }

    #[test]
    fn ids_that_could_escape_the_folder_are_refused() {
        let temp = Temp::new("ids");
        let store = Store::new(temp.0.clone());
        assert!(is_valid_id("01JAB3K9X7Q2M8ZZZZZZZZZZZZ"));
        for bad in ["", r"..\x", "a/b", "a b", &"A".repeat(65)] {
            assert!(!is_valid_id(bad), "{bad:?}");
            assert!(store.put(&job(bad, "1")).is_err());
            assert!(store.get(bad).is_none());
        }
    }

    #[test]
    fn raw_sources_without_a_job_get_one() {
        let temp = Temp::new("orphans");
        let notebook = temp.0.join("notebook");
        let raw = notebook.join("raw");
        fs::create_dir_all(&raw).unwrap();
        let source = |id: &str| {
            format!("---\ntype: raw\ncaptured: 2026-10-19T10:30\ncapture_id: {id}\n---\n\nx\n")
        };
        fs::write(raw.join("2026-10-19 1030 a.md"), source("AAA")).unwrap();
        fs::write(raw.join("2026-10-19 1031 b.md"), source("BBB")).unwrap();
        fs::write(
            raw.join("note.md"),
            "---\ntype: capture\ncapture_id: CCC\n---\n",
        )
        .unwrap();
        fs::write(raw.join("bad id.md"), source("../x")).unwrap();
        fs::write(raw.join("plain.md"), "no front matter").unwrap();
        fs::write(raw.join("a.png"), b"png").unwrap();

        let store = Store::new(temp.0.join("queue"));
        store.put(&job("BBB", "2026-10-19T10:31")).unwrap();
        let conventions = Conventions::load(&notebook);
        let added = store.enqueue_orphans(&conventions);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].capture_id, "AAA");
        assert_eq!(added[0].raw, PathBuf::from(r"raw\2026-10-19 1030 a.md"));
        assert_eq!(added[0].notebook, notebook);
        assert_eq!(added[0].created, "2026-10-19T10:30");
        assert!(store.enqueue_orphans(&conventions).is_empty(), "only once");
    }
}
