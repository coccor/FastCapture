//! What the review window lists: one row per capture, newest first, from the queue, the raw
//! sources and the notes written for them. Pure: the caller reads the files.

use crate::capture::read::{self, RawInfo};
use crate::note::write::Written;
use crate::notebook::dates::{self, Date};
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

/// The rows for `jobs`, newest first, reading each raw source with `read_file` (absolute path).
/// The second list holds the jobs whose raw source is gone.
pub fn rows(jobs: Vec<Job>, read_file: impl Fn(&Path) -> Option<String>) -> (Vec<Row>, Vec<Job>) {
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
    fn a_written_note_names_the_row_and_leads_the_preview() {
        use crate::note::write::Stamp;
        let files: HashMap<PathBuf, String> = [
            (
                PathBuf::from(r"D:\Notes\raw\a.md"),
                "---\ntype: raw\nkind: [text]\n---\n\nlag\n".to_owned(),
            ),
            (
                PathBuf::from(r"D:\Notes\inbox\Lag.md"),
                "---\ntype: capture\n---\n\n# Consumer lag spikes\n\n> Summary.\n".to_owned(),
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
        let (rows, _) = rows(vec![done, gone, discarded], |path| files.get(path).cloned());

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
        assert_eq!(
            (rows[1].title.as_str(), rows[1].status()),
            ("lag", "Note gone")
        );
        assert_eq!(rows[2].status(), "Discarded");
        assert!(!preview_text(&rows[2]).contains("Raw source"));
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
