//! What the review window lists, newest first: the notes FastCapture wrote that wait in the
//! inbox (`type: capture`, `status: inbox`, in the capture folder), and the captures still
//! queued, being written or failed. Filed, kept and discarded notes leave the list. Pure: the
//! caller reads the files.

use crate::capture::read::{self, RawInfo};
use crate::note::write::Written;
use crate::notebook::dates::{self, Date};
use crate::notebook::frontmatter::{self, Value};
use crate::queue::{Job, State};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    pub capture_id: String,
    pub notebook: PathBuf,
    /// Relative to the notebook.
    pub raw: PathBuf,
    pub title: String,
    pub kinds: String,
    pub created: String,
    pub state: State,
    pub error: Option<String>,
    /// The raw source's text, for the preview.
    pub text: String,
    pub info: RawInfo,
    /// The note written for it, as FastCapture last wrote it.
    pub note: Option<Written>,
    /// The note's text now, when it is still there.
    pub note_text: Option<String>,
    /// Where the note suggests it be filed, as written: `[[Payments incidents]]`,
    /// `Payments/Incidents/`.
    pub home: Option<String>,
}

impl Row {
    pub fn raw_path(&self) -> PathBuf {
        self.notebook.join(&self.raw)
    }

    pub fn note_path(&self) -> Option<PathBuf> {
        self.note
            .as_ref()
            .map(|note| self.notebook.join(&note.path))
    }

    /// What the Home column says: the suggestion without its brackets.
    pub fn home_label(&self) -> String {
        match &self.home {
            Some(home) => {
                let name = home
                    .strip_prefix("[[")
                    .and_then(|name| name.strip_suffix("]]"))
                    .unwrap_or(home);
                format!("\u{2192} {name}")
            }
            None if self.note_text.is_some() => "No home".to_owned(),
            None => String::new(),
        }
    }

    /// What the State column says.
    pub fn status(&self) -> &'static str {
        match (self.state, &self.note, &self.note_text) {
            (State::Done, Some(_), Some(_)) => "Note",
            (State::Done, Some(_), None) => "Note gone",
            (state, _, _) => state_label(state),
        }
    }
}

/// The note's title: its first `# ` heading.
fn note_title(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("# "))
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty())
}

/// Whether the note at `path` (relative to its notebook), whose text is `text`, waits in the
/// inbox: FastCapture wrote it, it is still `status: inbox`, and it is in `capture_folder`.
pub fn waits_in_inbox(path: &Path, text: &str, capture_folder: &Path) -> bool {
    let Some(front) = frontmatter::parse(text) else {
        return false;
    };
    let is = |key: &str, wanted: &str| matches!(front.get(key), Some(Value::Text(value)) if value.trim().eq_ignore_ascii_case(wanted));
    let folder = path.parent().unwrap_or(Path::new(""));
    is("type", "capture")
        && is("status", "inbox")
        && folder
            .to_string_lossy()
            .eq_ignore_ascii_case(&capture_folder.to_string_lossy())
}

/// The note's `suggested_home:`, as written.
fn suggested_home(text: &str) -> Option<String> {
    match frontmatter::parse(text)?.get("suggested_home")? {
        Value::Text(home) if !home.trim().is_empty() => Some(home.trim().to_owned()),
        _ => None,
    }
}

/// The rows for `jobs`, newest first, reading each raw source with `read_file` (absolute path)
/// and asking `capture_folder` where a notebook's notes wait. The second list holds the jobs
/// whose raw source is gone.
pub fn rows(
    jobs: Vec<Job>,
    read_file: impl Fn(&Path) -> Option<String>,
    capture_folder: impl Fn(&Path) -> PathBuf,
) -> (Vec<Row>, Vec<Job>) {
    let mut rows = Vec::new();
    let mut gone = Vec::new();
    for job in jobs {
        let Some(text) = read_file(&job.notebook.join(&job.raw)) else {
            gone.push(job);
            continue;
        };
        let info = read::read(&job.raw, &text);
        let note_text = job
            .note
            .as_ref()
            .and_then(|note| read_file(&job.notebook.join(&note.path)));
        let listed = match job.state {
            State::Queued | State::Working | State::Failed => true,
            State::Done => match (&job.note, &note_text) {
                (Some(note), Some(text)) => {
                    waits_in_inbox(&note.path, text, &capture_folder(&job.notebook))
                }
                _ => false,
            },
            State::Discarded => false,
        };
        if !listed {
            continue;
        }
        let home = note_text.as_deref().and_then(suggested_home);
        let title = note_text
            .as_deref()
            .and_then(note_title)
            .or_else(|| info.title.clone())
            .unwrap_or_else(|| {
                job.raw
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        rows.push(Row {
            kinds: kind_label(&info.kinds),
            capture_id: job.capture_id,
            notebook: job.notebook,
            raw: job.raw,
            title,
            created: job.created,
            state: job.state,
            error: job.last_error,
            text,
            info,
            note: job.note,
            note_text,
            home,
        });
    }
    rows.sort_by(|a, b| {
        (b.created.as_str(), b.capture_id.as_str())
            .cmp(&(a.created.as_str(), a.capture_id.as_str()))
    });
    (rows, gone)
}

/// `text + screenshot` from front matter kinds: "Text + screenshot".
pub fn kind_label(kinds: &[String]) -> String {
    let joined = kinds.join(" + ");
    let mut chars = joined.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `2026-10-19T10:30` as "10:30" on `today`, else "19 Oct 10:30", else as written.
pub fn time_label(created: &str, today: Date) -> String {
    let Some((day, time)) = created.split_once('T') else {
        return created.to_owned();
    };
    match dates::parse(day, "YYYY-MM-DD") {
        Some(date) if date == today => time.to_owned(),
        Some(date) if date.year == today.year => format!("{} {time}", dates::format(date, "D MMM")),
        Some(date) => format!("{} {time}", dates::format(date, "D MMM YYYY")),
        None => created.to_owned(),
    }
}

pub fn state_label(state: State) -> &'static str {
    match state {
        State::Queued => "Queued",
        State::Working => "Writing",
        State::Done => "Done",
        State::Failed => "Failed",
        State::Discarded => "Discarded",
    }
}

/// The preview pane's text: an error first (for a failed job, or one waiting to retry), then
/// the note when there is one, then the raw source, with CRLF line ends for the edit control.
pub fn preview_text(row: &Row) -> String {
    let mut text = String::new();
    if let Some(error) = &row.error
        && matches!(row.state, State::Failed | State::Queued)
    {
        text.push_str(&format!("Last error: {error}\n\n"));
    }
    if let Some(note) = &row.note_text {
        text.push_str(note.trim_end());
        text.push_str("\n\n──────── Raw source ────────\n\n");
    }
    text.push_str(&row.text);
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn job(id: &str, created: &str, raw: &str) -> Job {
        Job::queued(
            id,
            PathBuf::from(r"D:\Notes"),
            PathBuf::from(raw),
            created.to_owned(),
        )
    }

    #[test]
    fn rows_are_newest_first_and_jobs_without_a_raw_source_are_set_aside() {
        let files: HashMap<PathBuf, String> = [
            (
                PathBuf::from(r"D:\Notes\raw\a.md"),
                "---\ntype: raw\nkind: [text, link]\n---\n\nRust cancellation\nhttps://x.example\n"
                    .to_owned(),
            ),
            (
                PathBuf::from(r"D:\Notes\raw\b.md"),
                "---\ntype: raw\nkind: [file]\n---\n\n[report.pdf](../attachments/report.pdf)\n"
                    .to_owned(),
            ),
        ]
        .into();
        let mut failed = job("B", "2026-10-19T10:31", r"raw\b.md");
        failed.state = State::Failed;
        failed.last_error = Some("timeout".to_owned());
        let (rows, gone) = rows(
            vec![
                job("A", "2026-10-19T09:00", r"raw\a.md"),
                failed,
                job("C", "2026-10-19T11:00", r"raw\gone.md"),
            ],
            |path| files.get(path).cloned(),
            |_| PathBuf::from("inbox"),
        );
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].capture_id, "C");
        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["report.pdf", "Rust cancellation"]);
        assert_eq!(rows[0].kinds, "File");
        assert_eq!(rows[1].kinds, "Text + link");
        assert_eq!(rows[0].raw_path(), PathBuf::from(r"D:\Notes\raw\b.md"));
        assert!(preview_text(&rows[0]).starts_with("Last error: timeout\r\n\r\n---\r\ntype: raw"));
    }

    #[test]
    fn inbox_notes_are_listed_and_filed_kept_or_discarded_ones_are_not() {
        use crate::note::write::Stamp;
        let files: HashMap<PathBuf, String> = [
            (
                PathBuf::from(r"D:\Notes\raw\a.md"),
                "---\ntype: raw\nkind: [text]\n---\n\nlag\n".to_owned(),
            ),
            (
                PathBuf::from(r"D:\Notes\inbox\Lag.md"),
                "---\ntype: capture\nsuggested_home: \"[[Payments incidents]]\"\nstatus: inbox\n---\n\n# Consumer lag spikes\n\n> Summary.\n".to_owned(),
            ),
            (
                PathBuf::from(r"D:\Notes\Payments\Filed.md"),
                "---\ntype: capture\nstatus: filed\n---\n# Filed\n".to_owned(),
            ),
            (
                PathBuf::from(r"D:\Notes\inbox\Kept.md"),
                "---\ntype: capture\nstatus: kept\n---\n# Kept\n".to_owned(),
            ),
            (
                PathBuf::from(r"D:\Notes\inbox\Plain.md"),
                "---\nstatus: inbox\n---\n# Not ours\n".to_owned(),
            ),
        ]
        .into();
        let note = Written {
            path: PathBuf::from(r"inbox\Lag.md"),
            stamp: Stamp {
                len: 1,
                modified_ms: 1,
            },
        };
        let mut done = job("A", "2026-10-19T09:00", r"raw\a.md");
        done.state = State::Done;
        done.note = Some(note.clone());
        done.last_error = Some("an old retry".to_owned());
        let mut gone = job("B", "2026-10-19T08:00", r"raw\a.md");
        gone.state = State::Done;
        gone.note = Some(Written {
            path: PathBuf::from(r"inbox\Missing.md"),
            ..note
        });
        let mut discarded = job("C", "2026-10-19T07:00", r"raw\a.md");
        discarded.state = State::Discarded;
        let elsewhere = |id: &str, path: &str| {
            let mut job = job(id, "2026-10-19T06:00", r"raw\a.md");
            job.state = State::Done;
            job.note = Some(Written {
                path: PathBuf::from(path),
                ..note
            });
            job
        };
        let (rows, _) = rows(
            vec![
                done,
                gone,
                discarded,
                elsewhere("D", r"Payments\Filed.md"),
                elsewhere("E", r"inbox\Kept.md"),
                elsewhere("F", r"inbox\Plain.md"),
            ],
            |path| files.get(path).cloned(),
            |_| PathBuf::from("Inbox"),
        );
        assert_eq!(rows.len(), 1, "only the note waiting in the inbox");

        assert_eq!(rows[0].title, "Consumer lag spikes");
        assert_eq!(rows[0].status(), "Note");
        assert_eq!(
            rows[0].note_path(),
            Some(PathBuf::from(r"D:\Notes\inbox\Lag.md"))
        );
        let preview = preview_text(&rows[0]);
        assert!(
            preview.starts_with("---\r\ntype: capture"),
            "no error for a done job: {preview}"
        );
        assert!(
            preview
                .contains("> Summary.\r\n\r\n──────── Raw source ────────\r\n\r\n---\r\ntype: raw")
        );
        assert_eq!(rows[0].home_label(), "\u{2192} Payments incidents");
    }

    #[test]
    fn times_are_short_for_today_and_dated_otherwise() {
        let today = Date::new(2026, 10, 19).unwrap();
        assert_eq!(time_label("2026-10-19T10:30", today), "10:30");
        assert_eq!(time_label("2026-10-18T09:05", today), "18 Oct 09:05");
        assert_eq!(time_label("2025-12-31T23:59", today), "31 Dec 2025 23:59");
        assert_eq!(time_label("soon", today), "soon");
        assert_eq!(state_label(State::Queued), "Queued");
        assert_eq!(kind_label(&[]), "");
    }
}
