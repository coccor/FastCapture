//! Notes coding agents write into a notebook's capture folder (`type: finding`, `status:
//! inbox`) get a job, so they are placed and reviewed like captures. A job knows its note by
//! path, so FastCapture never writes into a note to adopt it. A note that looks like a finding
//! but whose front matter can't be read gets a failed job that says so.

use super::job::{Job, State};
use super::store::Store;
use crate::note::write::{Stamp, Written};
use crate::notebook::Conventions;
use crate::notebook::frontmatter::{self, Value};
use std::path::{Path, PathBuf};

/// How long a note must go unchanged before it is adopted: an agent may still be writing it.
pub const SETTLE_MS: u64 = 2_000;

/// What a note in the capture folder is, read from its text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Finding {
    /// A finding waiting in the inbox, captured at this time (`YYYY-MM-DDTHH:MM`) when it says.
    Ready { captured: Option<String> },
    /// It says `type: finding`, but its front matter can't be read, and why.
    Unreadable(String),
}

/// Whether `text` is a finding waiting in the inbox, or one whose front matter can't be read.
/// `None` for anything else.
pub fn classify(text: &str) -> Option<Finding> {
    match frontmatter::parse(text) {
        Some(front) => {
            let is = |key: &str, wanted: &str| matches!(front.get(key), Some(Value::Text(value)) if value.trim().eq_ignore_ascii_case(wanted));
            if !(is("type", "finding") && is("status", "inbox")) {
                return None;
            }
            let captured = match front.get("captured") {
                Some(Value::Text(value)) if !value.trim().is_empty() => {
                    Some(value.trim().to_owned())
                }
                _ => None,
            };
            Some(Finding::Ready { captured })
        }
        None => {
            let mut lines = text.trim_start_matches('\u{feff}').lines();
            if lines.next().map(str::trim_end) != Some("---") {
                return None;
            }
            let says_finding = lines.take(frontmatter::MAX_LINES).any(|line| {
                let line = line.trim().to_lowercase();
                line == "type: finding" || line == "type: \"finding\"" || line == "type: 'finding'"
            });
            says_finding.then(|| {
                Finding::Unreadable(
                    "Its front matter can't be read: it must start with a --- line and end with \
                     another --- line within 200 lines."
                        .to_owned(),
                )
            })
        }
    }
}

/// Whether `a` and `b` name the same folder, whatever their case, separators or trailing
/// separator.
pub fn same_folder(a: &Path, b: &Path) -> bool {
    let key = |path: &Path| {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    key(a) == key(b)
}

/// The settled notes in `root`'s capture folder that are findings and have no job among `jobs`
/// get one, added to `jobs` and `store`, at `now` (milliseconds since 1970), stamped `captured`
/// when the note doesn't say. A job knows a note where it is now; a discarded finding's job
/// keeps only the very file it recycled, should it be put back. Returns the jobs added, and
/// whether a finding is still being written (so look again soon).
pub fn adopt(
    store: &Store,
    jobs: &mut Vec<Job>,
    root: &Path,
    now: u64,
    captured: &str,
) -> (Vec<Job>, bool) {
    let folder = Conventions::load(root).capture_folder().to_path_buf();
    let Ok(entries) = std::fs::read_dir(root.join(&folder)) else {
        return (Vec::new(), false);
    };
    let key = |path: &Path| path.to_string_lossy().to_lowercase();
    let known: Vec<String> = jobs
        .iter()
        .filter(|job| same_folder(&job.notebook, root))
        .filter_map(|job| {
            let note = job.note.as_ref()?;
            let recycled = job.finding && job.state == State::Discarded;
            (!recycled || Stamp::of(&root.join(&note.path)) == Some(note.stamp))
                .then(|| key(&note.path))
        })
        .collect();
    let mut added = Vec::new();
    let mut settling = false;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| folder.join(entry.file_name()))
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        })
        .filter(|path| !known.contains(&key(path)))
        .collect();
    paths.sort();
    for path in paths {
        let absolute = root.join(&path);
        let Some(stamp) = Stamp::of(&absolute) else {
            continue;
        };
        let Some(text) = super::store::read_head(&absolute) else {
            continue;
        };
        let Some(finding) = classify(&text) else {
            continue;
        };
        // Either way: a time far ahead of the clock (a note synced from a PC whose clock runs
        // fast) doesn't hold it back for good.
        if now.abs_diff(stamp.modified_ms) < SETTLE_MS {
            settling = true;
            continue;
        }
        let note = Written { path, stamp };
        let mut job = Job::finding(
            &crate::capture::ulid::new(),
            root.to_path_buf(),
            note,
            captured.to_owned(),
        );
        match finding {
            Finding::Ready { captured } => {
                if let Some(captured) = captured {
                    job.created = captured;
                }
            }
            Finding::Unreadable(problem) => {
                job.state = State::Failed;
                job.last_error = Some(problem);
            }
        }
        match store.put(&job) {
            Ok(()) => {
                jobs.push(job.clone());
                added.push(job);
            }
            Err(error) => crate::log::line(&format!("queue: {error}")),
        }
    }
    (added, settling)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINDING: &str = "---\ntype: finding\nfinding: dead-end\ncaptured: 2026-10-23T14:05\nauthor: claude-code\nsummary: \"Raising max.poll.records made the lag worse.\"\nstatus: inbox\n---\n\n# Raising max.poll.records makes the lag worse\n";

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "fastcapture-findings-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join(r"Notes\inbox")).unwrap();
            Self(dir)
        }

        fn notebook(&self) -> PathBuf {
            self.0.join("Notes")
        }

        fn store(&self) -> Store {
            Store::new(self.0.join("queue"))
        }

        fn write(&self, name: &str, text: &str) {
            std::fs::write(self.notebook().join("inbox").join(name), text).unwrap();
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Far enough after any file's write time for it to have settled.
    const LATER: u64 = u64::MAX / 2;

    #[test]
    fn findings_in_the_inbox_are_told_from_other_notes() {
        assert_eq!(
            classify(FINDING),
            Some(Finding::Ready {
                captured: Some("2026-10-23T14:05".to_owned())
            })
        );
        assert_eq!(
            classify(&FINDING.replace("status: inbox", "status: filed")),
            None
        );
        assert_eq!(
            classify(&FINDING.replace("type: finding", "type: capture")),
            None
        );
        assert_eq!(classify("# No front matter\n"), None);
        assert!(matches!(
            classify("---\ntype: finding\nstatus: inbox\n\n# Never closed\n"),
            Some(Finding::Unreadable(_))
        ));
        assert_eq!(classify("---\ntitle: never closed\n"), None);
    }

    impl Temp {
        /// Looks in the notebook, spelled `root`, at `now`, with the queue as it is.
        fn look_in(&self, root: &Path, now: u64) -> (Vec<Job>, bool) {
            let store = self.store();
            let mut jobs = store.list();
            adopt(&store, &mut jobs, root, now, "2026-10-23T15:00")
        }

        fn look(&self, now: u64) -> (Vec<Job>, bool) {
            self.look_in(&self.notebook(), now)
        }

        fn stamp(&self, name: &str) -> Stamp {
            Stamp::of(&self.notebook().join("inbox").join(name)).unwrap()
        }
    }

    #[test]
    fn a_settled_finding_gets_one_job_and_an_unreadable_one_a_failed_job() {
        let temp = Temp::new("adopt");
        temp.write("Lag.md", FINDING);
        temp.write("Broken.md", "---\ntype: finding\nstatus: inbox\n# Broken\n");
        temp.write(
            "Plain.md",
            "---\ntype: capture\nstatus: inbox\n---\n# Plain\n",
        );
        let (added, settling) = temp.look(LATER);
        assert!(!settling);
        let summary: Vec<(String, State, String)> = added
            .iter()
            .map(|job| {
                (
                    job.raw.to_string_lossy().into_owned(),
                    job.state,
                    job.created.clone(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    r"inbox\Broken.md".to_owned(),
                    State::Failed,
                    "2026-10-23T15:00".to_owned()
                ),
                (
                    r"inbox\Lag.md".to_owned(),
                    State::Queued,
                    "2026-10-23T14:05".to_owned()
                ),
            ]
        );
        assert!(added.iter().all(|job| job.finding));
        assert!(
            added[0]
                .last_error
                .as_deref()
                .unwrap()
                .contains("front matter")
        );
        assert_eq!(
            added[1].note.as_ref().map(|note| note.path.clone()),
            Some(PathBuf::from(r"inbox\Lag.md"))
        );
        // The note itself is left as the agent wrote it.
        assert_eq!(
            std::fs::read_to_string(temp.notebook().join(r"inbox\Lag.md")).unwrap(),
            FINDING
        );
        // A second look adds nothing, nor does the notebook spelled another way.
        assert!(temp.look(LATER).0.is_empty());
        let shouted = PathBuf::from(format!(
            "{}\\",
            temp.notebook().to_string_lossy().to_uppercase()
        ));
        assert!(temp.look_in(&shouted, LATER).0.is_empty());
        assert_eq!(temp.store().list().len(), 2);
    }

    #[test]
    fn one_look_at_two_spellings_of_a_notebook_adopts_once() {
        let temp = Temp::new("spellings");
        temp.write("Lag.md", FINDING);
        let store = temp.store();
        let mut jobs = store.list();
        let shouted = PathBuf::from(temp.notebook().to_string_lossy().to_uppercase());
        let (first, _) = adopt(&store, &mut jobs, &temp.notebook(), LATER, "x");
        let (second, _) = adopt(&store, &mut jobs, &shouted, LATER, "x");
        assert_eq!((first.len(), second.len()), (1, 0));
    }

    #[test]
    fn a_finding_still_being_written_waits_but_one_from_the_future_doesnt() {
        let temp = Temp::new("settling");
        temp.write("Lag.md", FINDING);
        let written = temp.stamp("Lag.md").modified_ms;
        let (added, settling) = temp.look(written + 500);
        assert!(added.is_empty() && settling);
        let (added, settling) = temp.look(written - 500);
        assert!(added.is_empty() && settling, "a clock a little behind");
        let (added, settling) = temp.look(written - 3_600_000);
        assert_eq!(added.len(), 1, "written an hour ahead of this clock");
        assert!(!settling);
    }

    #[test]
    fn a_new_finding_named_like_a_filed_one_is_adopted() {
        let temp = Temp::new("filed");
        temp.write("Lag.md", FINDING);
        let (added, _) = temp.look(LATER);
        // Filed: the note moved, and its job with it.
        let filed = temp.notebook().join(r"Payments\Lag.md");
        std::fs::create_dir_all(filed.parent().unwrap()).unwrap();
        std::fs::rename(temp.notebook().join(r"inbox\Lag.md"), &filed).unwrap();
        let mut job = added[0].clone();
        job.state = State::Done;
        job.note = Some(Written {
            path: PathBuf::from(r"Payments\Lag.md"),
            stamp: Stamp::of(&filed).unwrap(),
        });
        temp.store().put(&job).unwrap();
        // Weeks later, a finding with the same title.
        temp.write("Lag.md", &FINDING.replace("worse.", "worse again."));
        let (again, _) = temp.look(LATER);
        assert_eq!(again.len(), 1);
        assert_ne!(again[0].capture_id, job.capture_id);
    }

    #[test]
    fn a_discarded_finding_put_back_stays_discarded_but_a_new_one_by_its_name_is_adopted() {
        let temp = Temp::new("discarded");
        temp.write("Lag.md", FINDING);
        let (added, _) = temp.look(LATER);
        let mut job = added[0].clone();
        job.state = State::Discarded;
        temp.store().put(&job).unwrap();
        // The very file, back from the Recycle Bin.
        assert!(temp.look(LATER).0.is_empty());
        // Another finding under the same name.
        temp.write("Lag.md", &FINDING.replace("worse.", "much worse."));
        assert_ne!(Some(temp.stamp("Lag.md")), job.note.map(|note| note.stamp));
        assert_eq!(temp.look(LATER).0.len(), 1);
    }
}
